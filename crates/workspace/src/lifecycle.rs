// Copyright (c) 2026 Windsor Nguyen

//! Retire managed leaves only while their recorded directory and check ownership match.

use std::{
    fs::{self, File},
    os::unix::fs::MetadataExt,
    path::Path,
    sync::Arc,
};

use cowtree_metadata::LeafId;
use serde::{Deserialize, Serialize};

use crate::{Error, Leaf, Result, Workspace, error::Issue, paths, records, session::Session};

#[derive(Clone, Copy)]
pub enum DropPolicy {
    RequireClean,
    DiscardPrivate,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DropRecord {
    /// Complete ownership witness retained until both filesystem and authority are retired.
    pub leaf: Leaf,
}

impl Workspace {
    pub fn drop_leaf(&self, identity: LeafId, policy: DropPolicy) -> Result<()> {
        let mut session = self.lock()?;
        let leaf = session.read_leaf(identity)?;
        if leaf.pending.is_some() {
            return Err(Issue::PendingPublication.at(&leaf.path));
        }
        if matches!(policy, DropPolicy::RequireClean) && session.current(&leaf)? != leaf.origins {
            return Err(Issue::SourceChanged.at(&leaf.path));
        }
        session.begin_drop(leaf)?;
        session.pin_origins()
    }

    pub(crate) fn retire_check(&self, identity: LeafId) -> Result<()> {
        let mut session = self.lock()?;
        if !self.root.join("leaves").join(format!("{}.json", identity.get())).exists()
            && !session.authority.leaves()?.contains(&identity)
        {
            return Ok(());
        }
        let leaf = session.read_leaf(identity)?;
        if leaf.check_candidate.is_none() {
            return Err(Issue::InvalidCandidate.at(&leaf.path));
        }
        session.begin_drop(leaf)?;
        session.pin_origins()
    }
}

impl Session {
    pub fn check_guard(&self, leaf: &Leaf) -> Result<Option<Arc<File>>> {
        let Some(candidate) = &leaf.check_candidate else {
            return Ok(None);
        };
        let path = self.workspace.check_lock(candidate);
        let lock = File::options()
            .read(true)
            .append(true)
            .create(true)
            .open(&path)
            .map_err(|error| Error::io(&path, error))?;
        match fs4::FileExt::try_lock(&lock) {
            Ok(()) => Ok(Some(Arc::new(lock))),
            Err(fs4::TryLockError::WouldBlock) => Err(Issue::ValidationBusy.at(&leaf.path)),
            Err(fs4::TryLockError::Error(error)) => Err(Error::io(&path, error)),
        }
    }

    pub fn begin_drop(&mut self, leaf: Leaf) -> Result<()> {
        let guard = self.check_guard(&leaf)?;
        let directory = self
            .workspace
            .root
            .join("operations")
            .join(format!("drop-{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir(&directory).map_err(|error| Error::io(&directory, error))?;
        let record = DropRecord { leaf };
        records::write(&directory.join("drop.json"), &record)?;
        self.finish_drop(&directory, &record, guard)
    }

    fn finish_drop(
        &mut self,
        directory: &Path,
        record: &DropRecord,
        guard: Option<Arc<File>>,
    ) -> Result<()> {
        let leaf = &record.leaf;
        if paths::exists(&leaf.path)? {
            let identity =
                fs::symlink_metadata(&leaf.path).map_err(|error| Error::io(&leaf.path, error))?;
            if !identity.is_dir() || (identity.dev(), identity.ino()) != (leaf.device, leaf.inode) {
                return Err(Issue::ChangedDirectory.at(&leaf.path));
            }
        }
        if self.authority.leaves()?.contains(&leaf.id) {
            self.authority.drop_leaf(leaf.id)?;
        }
        let mut repository = self.repository().locked()?;
        repository.locks.extend(guard);
        if repository.has_worktree(&leaf.path)? {
            repository.capture(
                repository
                    .command()?
                    .args(["worktree", "remove", "--force", "--force", "--"])
                    .arg(&leaf.path),
            )?;
        } else if leaf.path.exists() {
            fs::remove_dir_all(&leaf.path).map_err(|error| Error::io(&leaf.path, error))?;
        }
        records::sync_directory(
            leaf.path.parent().ok_or_else(|| Issue::InvalidRoot.at(&leaf.path))?,
        )?;
        let record_path =
            self.workspace.root.join("leaves").join(format!("{}.json", leaf.id.get()));
        match fs::remove_file(&record_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(Error::io(&record_path, error)),
        }
        records::sync_directory(&self.workspace.root.join("leaves"))?;
        records::remove_directory(directory)
    }

    pub fn recover_drops(&mut self) -> Result<()> {
        for directory in paths::entries(&self.workspace.root.join("operations"))? {
            if !directory
                .file_name()
                .is_some_and(|name| name.as_encoded_bytes().starts_with(b"drop-"))
            {
                continue;
            }
            if directory.join("drop.json").exists() {
                let record: DropRecord = records::read(&directory.join("drop.json"))?;
                let guard = self.check_guard(&record.leaf)?;
                self.finish_drop(&directory, &record, guard)?;
            } else {
                records::remove_directory(&directory)?;
            }
        }
        Ok(())
    }
}
