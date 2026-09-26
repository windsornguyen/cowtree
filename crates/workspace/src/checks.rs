// Copyright (c) 2026 Windsor Nguyen

//! Check the exact prepared candidate in a private warm worktree.
//!
//! A separate native supervisor retains the check lock through coordinator death.
//! Source changes invalidate the check. Successful checks seal warmed cache bytes.

use std::{
    fs::File,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
};

use std::os::unix::process::CommandExt;

use cowtree_metadata::{Candidate, LeafId, ResourcePath};

use crate::{
    Error, Leaf, Pending, Result, Validation, Workspace,
    error::Issue,
    installation::{Direction, Installation},
    nodes::Seal,
    session::Session,
    views::install_record,
};

#[derive(Clone, Debug)]
pub struct CheckRequest {
    /// Native supervisor executable supplied by the current caller.
    pub supervisor: PathBuf,
    /// Leaf whose exact pending candidate must be checked.
    pub identity: LeafId,
    /// Validation argv, never interpreted by a shell implicitly.
    pub command: Vec<String>,
    /// Positive wall-clock limit enforced by the native supervisor.
    pub timeout_seconds: u64,
}

impl Workspace {
    pub fn check(&self, request: &CheckRequest) -> Result<Validation> {
        if request.command.is_empty() || request.timeout_seconds == 0 {
            return Err(Issue::InvalidCandidate.at(&self.root));
        }
        let pending = self
            .lock()?
            .read_leaf(request.identity)?
            .pending
            .filter(|pending| pending.candidate.is_some() && pending.parent_node.is_some())
            .ok_or_else(|| Issue::InvalidCandidate.at(&self.root))?;
        let candidate =
            pending.candidate.as_ref().ok_or_else(|| Issue::IncompleteRecord.at(&self.root))?;
        let lock_path = self.check_lock(candidate);
        let lock = File::options()
            .read(true)
            .append(true)
            .create(true)
            .open(&lock_path)
            .map_err(|error| Error::io(&lock_path, error))?;
        match fs4::FileExt::try_lock(&lock) {
            Ok(()) => {}
            Err(fs4::TryLockError::WouldBlock) => return Err(Issue::ValidationBusy.at(&lock_path)),
            Err(fs4::TryLockError::Error(error)) => return Err(Error::io(&lock_path, error)),
        }
        let lock = Arc::new(lock);
        let target = self
            .root
            .parent()
            .ok_or_else(|| Issue::InvalidRoot.at(&self.root))?
            .join(format!(".cowtree-check-{}", uuid::Uuid::new_v4().simple()));
        let leaf =
            self.fork_check(&target, pending.parent_node.clone(), pending.candidate.clone())?;
        let checked = self.lock()?.materialize_check(leaf, &pending, lock.clone())?;
        let log = self.root.join("checks").join(format!("{}.log", checked.id.get()));
        self.run_check(request, &checked, &log, &lock)?;
        let result = self.lock()?.seal_check(request, &checked, &pending, &log)?;
        drop(lock);
        // Retirement follows after the validation lock is released.
        self.retire_check(checked.id)?;
        Ok(result)
    }

    pub(crate) fn check_lock(&self, candidate: &Candidate) -> PathBuf {
        self.root.join("checks").join(format!(
            "{}-{}.lock",
            candidate.request.leaf.get(),
            candidate.request.sequence
        ))
    }

    fn run_check(
        &self,
        request: &CheckRequest,
        leaf: &Leaf,
        log: &Path,
        lock: &File,
    ) -> Result<()> {
        let output = File::options()
            .write(true)
            .create_new(true)
            .open(log)
            .map_err(|error| Error::io(log, error))?;
        let mut command = Command::new(&request.supervisor);
        command
            .process_group(0)
            .args(["__supervise", "--timeout", &request.timeout_seconds.to_string(), "--"])
            .args(&request.command)
            .current_dir(&leaf.path)
            .stdin(Stdio::from(lock.try_clone().map_err(|error| Error::io(log, error))?))
            .stdout(output.try_clone().map_err(|error| Error::io(log, error))?)
            .stderr(output.try_clone().map_err(|error| Error::io(log, error))?);
        let status = command.status().map_err(|error| Error::io(&request.supervisor, error))?;
        output.sync_all().map_err(|error| Error::io(log, error))?;
        if !status.success() {
            return Err(Error::Check { status, log: log.into() });
        }
        Ok(())
    }
}

impl Session {
    fn materialize_check(
        &mut self,
        mut leaf: Leaf,
        pending: &Pending,
        lock: Arc<File>,
    ) -> Result<Leaf> {
        let candidate =
            pending.candidate.as_ref().ok_or_else(|| Issue::IncompleteRecord.at(&leaf.path))?;
        let current = self
            .read_leaf(pending.request.leaf)?
            .pending
            .ok_or_else(|| Issue::InvalidCandidate.at(&leaf.path))?;
        if current.candidate != pending.candidate {
            return Err(Issue::InvalidCandidate.at(&leaf.path));
        }
        let manifest = self.candidate_manifest(candidate)?;
        let original = self.current(&leaf)?;
        let selected: std::collections::BTreeSet<&ResourcePath> =
            original.keys().chain(manifest.keys()).collect();
        let changes = selected
            .into_iter()
            .filter(|path| original.get(*path) != manifest.get(*path))
            .map(|path| crate::installation::Change {
                path: path.clone(),
                before: original.get(path).cloned(),
                after: manifest.get(path).cloned(),
            })
            .collect();
        let directory =
            self.workspace.root.join("checks").join(format!("{}-install", leaf.id.get()));
        let install = Installation::prepare(
            &directory,
            install_record(&leaf.path, changes),
            &self.authority.object_directory(),
        )?;
        install.apply(Direction::Apply)?;
        let mut repository = self.repository();
        repository.locks.push(lock);
        let parent = self.nodes().read(
            pending.parent_node.as_ref().ok_or_else(|| Issue::IncompleteRecord.at(&leaf.path))?,
        )?;
        let commit = repository.project(&leaf.path, &manifest, &[parent.git_commit], None)?;
        let target = repository.select(&leaf.path);
        target.detached(std::slice::from_ref(&leaf.git_head))?;
        target.reset_index(&commit)?;
        leaf.origins = manifest;
        leaf.git_head = commit;
        self.save_leaf(&leaf)?;
        install.finish()?;
        Ok(leaf)
    }

    fn seal_check(
        &mut self,
        request: &CheckRequest,
        leaf: &Leaf,
        pending: &Pending,
        log: &Path,
    ) -> Result<Validation> {
        let mut owner = self.read_leaf(request.identity)?;
        let current = owner
            .pending
            .as_mut()
            .filter(|current| current.candidate == pending.candidate && !current.aborting)
            .ok_or_else(|| Issue::InvalidCandidate.at(&owner.path))?;
        let candidate =
            pending.candidate.as_ref().ok_or_else(|| Issue::IncompleteRecord.at(&owner.path))?;
        let manifest = self.candidate_manifest(candidate)?;
        self.validate_leaf(leaf)?;
        let node = self.nodes().seal(
            &leaf.path,
            Seal {
                origins: Some(manifest.clone()),
                parent: pending.parent_node.clone(),
                reuse: Some(leaf.git_head.clone()),
                ..Seal::default()
            },
        )?;
        if node.source != manifest {
            return Err(Issue::SourceChanged.at(&leaf.path));
        }
        self.nodes().publish(&node)?;
        let validation = Validation {
            candidate: candidate.clone(),
            command: request.command.clone(),
            log: log.into(),
            node: node.id,
        };
        current.validation = Some(validation.clone());
        self.save_leaf(&owner)?;
        self.pin_origins()?;
        Ok(validation)
    }
}
