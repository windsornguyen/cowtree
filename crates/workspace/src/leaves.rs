// Copyright (c) 2026 Windsor Nguyen

//! Reserve private worktree identities before cloning outside the workspace lock.
//!
//! An operation lock pins the frozen source while population runs. Recovery only
//! retires a fork after that lock becomes available and ownership still matches.

use std::{
    collections::BTreeSet,
    fs::{self, File},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::Arc,
};

use cowtree::{CaptureMode, TreeKind};
use cowtree_metadata::{Candidate, LeafId};
use serde::{Deserialize, Serialize};

use crate::{
    Error, Leaf, Node, NodeId, Result, Workspace, durability, error::Issue, manifest, paths,
    records, session::Session, workspace::canonical_target,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ForkRecord {
    /// Exclusively reserved destination.
    pub path: PathBuf,
    /// Frozen image retained by this operation.
    pub node: NodeId,
    /// Authority identities before the allocation, for a lost allocation reply.
    pub before: Vec<LeafId>,
    /// Device of the reservation directory.
    pub device: u64,
    /// Inode of the reservation directory.
    pub inode: u64,
    /// Allocated authority identity once acknowledged in this journal.
    pub leaf: Option<LeafId>,
    /// Candidate pin when this fork belongs to validation.
    pub check_candidate: Option<Candidate>,
}

impl Workspace {
    pub fn fork(&self, path: &Path, node: Option<NodeId>) -> Result<Leaf> {
        self.fork_check(path, node, None)
    }

    pub(crate) fn fork_check(
        &self,
        path: &Path,
        node: Option<NodeId>,
        check: Option<Candidate>,
    ) -> Result<Leaf> {
        let (directory, lock, record, snapshot) = {
            let mut session = self.lock()?;
            let target = session.fork_target(path)?;
            let snapshot = session
                .nodes()
                .read(node.as_ref().unwrap_or(&session.workspace.config.warm_tip))?;
            let directory = self
                .root
                .join("operations")
                .join(format!("fork-{}", uuid::Uuid::new_v4().simple()));
            fs::create_dir(&directory).map_err(|error| Error::io(&directory, error))?;
            let lock_path = directory.join("lock");
            let lock = File::options()
                .read(true)
                .append(true)
                .create(true)
                .open(&lock_path)
                .map_err(|error| Error::io(&lock_path, error))?;
            fs4::FileExt::lock(&lock).map_err(|error| Error::io(&lock_path, error))?;
            let lock = Arc::new(lock);
            let record =
                session.prepare_fork(target, &snapshot, &directory, lock.clone(), check)?;
            (directory, lock, record, snapshot)
        };
        let source = self.root.join("nodes").join(snapshot.id.as_str()).join("tree");
        crate::fault::checkpoint("before-fork-population");
        cowtree::populate_tree(
            &source,
            &record.path,
            &snapshot.policy.compile()?,
            CaptureMode::Metadata,
        )?;
        let mut session = self.lock()?;
        let leaf = session.finish_fork(&record, &snapshot, lock)?;
        records::remove_directory(&directory)?;
        session.pin_origins()?;
        Ok(leaf)
    }
}

impl Session {
    fn fork_target(&self, path: &Path) -> Result<PathBuf> {
        if paths::exists(path)? {
            return Err(Issue::InvalidRoot.at(path));
        }
        let target = canonical_target(path)?;
        let parent = target.parent().ok_or_else(|| Issue::InvalidRoot.at(&target))?;
        let device = fs::metadata(&self.workspace.root)
            .map_err(|error| Error::io(&self.workspace.root, error))?
            .dev();
        if fs::metadata(parent).map_err(|error| Error::io(parent, error))?.dev() != device {
            return Err(Error::io(&target, std::io::ErrorKind::CrossesDevices.into()));
        }
        let mut roots = self.repository().worktrees()?;
        roots.push(self.workspace.root.clone());
        for parent in target.ancestors().skip(1) {
            let identity = fs::metadata(parent).map_err(|error| Error::io(parent, error))?;
            for root in &roots {
                match fs::metadata(root) {
                    Ok(metadata)
                        if (metadata.dev(), metadata.ino()) == (identity.dev(), identity.ino()) =>
                    {
                        return Err(Issue::InvalidRoot.at(&target));
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(Error::io(root, error)),
                }
            }
        }
        Ok(target)
    }

    fn prepare_fork(
        &mut self,
        target: PathBuf,
        node: &Node,
        directory: &Path,
        lock: Arc<File>,
        check: Option<Candidate>,
    ) -> Result<ForkRecord> {
        let reserved = directory.join("reserved");
        fs::create_dir(&reserved).map_err(|error| Error::io(&reserved, error))?;
        let identity = fs::metadata(&reserved).map_err(|error| Error::io(&reserved, error))?;
        let mut record = ForkRecord {
            path: target,
            node: node.id.clone(),
            before: self.authority.leaves()?,
            device: identity.dev(),
            inode: identity.ino(),
            leaf: None,
            check_candidate: check,
        };
        records::write(&directory.join("fork.json"), &record)?;
        let leaf = self.authority.create_leaf()?;
        crate::fault::checkpoint("after-leaf-allocation");
        record.leaf = Some(leaf);
        records::write(&directory.join("fork.json"), &record)?;
        durability::publish_directory(&reserved, &record.path)?;
        let mut repository = self.repository().locked()?;
        repository.locks.push(lock);
        repository.capture(
            repository
                .command()?
                .args([
                    "-c",
                    "core.fsync=all",
                    "worktree",
                    "add",
                    "--no-checkout",
                    "--detach",
                    "--lock",
                    "--reason",
                    &format!("cowtree managed leaf {}", leaf.get()),
                    "--",
                ])
                .arg(&record.path)
                .arg(node.git_commit.as_str()),
        )?;
        Ok(record)
    }

    fn finish_fork(&mut self, record: &ForkRecord, node: &Node, lock: Arc<File>) -> Result<Leaf> {
        check_identity(record)?;
        let id = record.leaf.ok_or_else(|| Issue::IncompleteRecord.at(&record.path))?;
        if !self.authority.leaves()?.contains(&id) {
            return Err(Issue::IncompleteRecord.at(&record.path));
        }
        let entries =
            cowtree::scan_tree(&record.path, &node.policy.compile()?, CaptureMode::Content)?;
        if manifest::capture(&record.path, &entries, &node.policy)? != node.source {
            return Err(Issue::SourceChanged.at(&record.path));
        }
        let mut repository = self.repository().locked()?;
        repository.locks.push(lock);
        let target = repository.select(&record.path);
        target.detached(std::slice::from_ref(&node.git_commit))?;
        target.reset_index(&node.git_commit)?;
        if target.head()? != node.git_commit || !target.status()?.is_empty() {
            return Err(Issue::SourceChanged.at(&record.path));
        }
        let mut files: Vec<_> = entries
            .iter()
            .filter(|entry| entry.kind == TreeKind::File)
            .map(|entry| record.path.join(&entry.path))
            .collect();
        files.push(record.path.join(".git"));
        let mut directories: Vec<_> = entries
            .iter()
            .rev()
            .filter(|entry| entry.kind == TreeKind::Directory)
            .map(|entry| record.path.join(&entry.path))
            .collect();
        directories.push(record.path.clone());
        durability::sync_tree(
            record.path.parent().ok_or_else(|| Issue::InvalidRoot.at(&record.path))?,
            files.iter().map(PathBuf::as_path),
            directories.iter().map(PathBuf::as_path),
        )?;
        let leaf = Leaf {
            id,
            path: record.path.clone(),
            device: record.device,
            inode: record.inode,
            node: node.id.clone(),
            git_head: node.git_commit.clone(),
            origins: node.origins.clone(),
            grants: Default::default(),
            sequence: std::num::NonZeroU64::MIN,
            pending: None,
            check_candidate: record.check_candidate.clone(),
            last_receipt: None,
        };
        self.save_leaf(&leaf)?;
        Ok(leaf)
    }

    pub fn recover_forks(&mut self) -> Result<Vec<LeafId>> {
        let mut recovered = Vec::new();
        for directory in paths::entries(&self.workspace.root.join("operations"))? {
            if !directory
                .file_name()
                .is_some_and(|name| name.as_encoded_bytes().starts_with(b"fork-"))
            {
                continue;
            }
            let path = directory.join("lock");
            let lock = File::options()
                .read(true)
                .append(true)
                .create(true)
                .open(&path)
                .map_err(|error| Error::io(&path, error))?;
            match fs4::FileExt::try_lock(&lock) {
                Ok(()) => {}
                Err(fs4::TryLockError::WouldBlock) => continue,
                Err(fs4::TryLockError::Error(error)) => return Err(Error::io(&path, error)),
            }
            if directory.join("fork.json").exists() {
                let record = records::read(&directory.join("fork.json"))?;
                if let Some(leaf) = self.abort_fork(&record, Arc::new(lock))? {
                    recovered.push(leaf);
                }
            }
            records::remove_directory(&directory)?;
        }
        Ok(recovered)
    }

    fn abort_fork(&mut self, record: &ForkRecord, lock: Arc<File>) -> Result<Option<LeafId>> {
        let active = self.authority.leaves()?;
        let leaf = match record.leaf {
            Some(leaf) => Some(leaf),
            None => {
                let previous: BTreeSet<_> = record.before.iter().copied().collect();
                let unknown: Vec<_> =
                    active.iter().filter(|leaf| !previous.contains(leaf)).copied().collect();
                if unknown.len() > 1 {
                    return Err(Issue::IncompleteRecord.at(&record.path));
                }
                unknown.first().copied()
            }
        };
        if let Some(id) = leaf {
            if self.workspace.root.join("leaves").join(format!("{}.json", id.get())).exists() {
                let ready = self.read_leaf(id)?;
                if ready.path != record.path || ready.node != record.node {
                    return Err(Issue::IncompleteRecord.at(&record.path));
                }
                return Ok(Some(id));
            }
        }
        if paths::exists(&record.path)? {
            check_identity(record)?;
            let mut repository = self.repository().locked()?;
            repository.locks.push(lock);
            if repository.has_worktree(&record.path)? {
                repository.capture(
                    repository
                        .command()?
                        .args(["worktree", "remove", "--force", "--force", "--"])
                        .arg(&record.path),
                )?;
            } else {
                fs::remove_dir_all(&record.path).map_err(|error| Error::io(&record.path, error))?;
            }
            records::sync_directory(
                record.path.parent().ok_or_else(|| Issue::InvalidRoot.at(&record.path))?,
            )?;
        }
        if let Some(leaf) = leaf {
            if active.contains(&leaf) {
                self.authority.drop_leaf(leaf)?;
            }
        }
        Ok(None)
    }
}

fn check_identity(record: &ForkRecord) -> Result<()> {
    let metadata =
        fs::symlink_metadata(&record.path).map_err(|error| Error::io(&record.path, error))?;
    if !metadata.is_dir() || (metadata.dev(), metadata.ino()) != (record.device, record.inode) {
        return Err(Issue::ChangedDirectory.at(&record.path));
    }
    Ok(())
}
