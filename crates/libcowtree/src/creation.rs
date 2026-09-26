// Copyright (c) 2026 Windsor Nguyen

//! Standalone creation owns its directory reservation and any newly created ref.
//!
//! Hold the common repository lock, pin a clean source, reserve an absent target,
//! register it with Git, and populate through the native batch API. Return only
//! after Git verifies the target against the pinned commit. Failure retires the
//! owned registration before conditionally deleting the ref and empty parents.

use crate::{
    AddRequest, Branch, Lock, RequestIssue, SourceMode, Worktree, WorktreeError as Error,
    WorktreeResult as Result,
    git::{Git, Snapshot},
    inspect_path, populate_tracked,
};
use std::{
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
};

pub fn add_worktree(request: &AddRequest) -> Result<Worktree> {
    PreparedAdd::new(request)?.run()
}

/// Hold the repository lock before a CLI installs cancellation handling.
pub struct PreparedAdd {
    repository: Git,
    request: AddRequest,
    _lock: std::sync::Arc<fs::File>,
}

impl PreparedAdd {
    pub fn new(request: &AddRequest) -> Result<Self> {
        validate(request)?;
        request.cancellation.check()?;
        let repository = Git::discover(request.source.as_deref())?;
        let lock = repository.lock()?;
        Ok(Self { repository, request: request.clone(), _lock: lock })
    }

    pub fn run(self) -> Result<Worktree> {
        self.request.cancellation.check()?;
        create(&self.repository, &self.request)
    }
}

fn validate(request: &AddRequest) -> Result<()> {
    if request.path.as_os_str().is_empty() || request.revision.is_empty() {
        return Err(Error::InvalidRequest { reason: RequestIssue::EmptyInput });
    }
    if let Lock::Retain { reason: Some(reason) } = &request.lock {
        if reason.is_empty() || reason.contains('\0') {
            return Err(Error::InvalidRequest { reason: RequestIssue::InvalidReason });
        }
    }
    if matches!(request.branch, Branch::Existing(_))
        && request.source_mode == SourceMode::Committed
        && request.revision != "HEAD"
    {
        return Err(Error::InvalidRequest { reason: RequestIssue::BranchSelectsRevision });
    }
    Ok(())
}

fn create(repository: &Git, request: &AddRequest) -> Result<Worktree> {
    Creation::new(repository, request)?.execute()
}

fn prepare(repository: &Git, request: &AddRequest) -> Result<(PathBuf, Snapshot)> {
    let target = resolve_target(&request.path)?;
    let snapshot = match request.source_mode {
        SourceMode::Checkout => {
            let snapshot = repository.snapshot()?;
            if request.revision != "HEAD"
                && repository.resolve(&request.revision)? != snapshot.commit
            {
                return Err(Error::HeadMismatch);
            }
            snapshot
        }
        SourceMode::Committed => {
            let revision = match &request.branch {
                Branch::Existing(branch) => qualified(branch),
                _ => request.revision.clone(),
            };
            let commit = repository.resolve(&revision)?;
            repository.tree(commit)?
        }
    };
    check_branch(repository, request, &snapshot)?;
    Ok((target, snapshot))
}

fn resolve_target(path: &Path) -> Result<PathBuf> {
    match fs::symlink_metadata(path) {
        Ok(_) => return Err(Error::DestinationExists { path: path.into() }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(Error::io(path, error)),
    }
    let absolute = std::path::absolute(path).map_err(|error| Error::io(path, error))?;
    let mut missing = Vec::new();
    let mut ancestor = absolute.as_path();
    while !ancestor.exists() {
        missing.push(
            ancestor
                .file_name()
                .ok_or(Error::InvalidRequest { reason: RequestIssue::InvalidDestination })?,
        );
        ancestor = ancestor
            .parent()
            .ok_or(Error::InvalidRequest { reason: RequestIssue::InvalidDestination })?;
    }
    let mut resolved = ancestor.canonicalize().map_err(|error| Error::io(ancestor, error))?;
    for name in missing.into_iter().rev() {
        resolved.push(name);
    }
    Ok(resolved)
}

fn check_branch(repository: &Git, request: &AddRequest, snapshot: &Snapshot) -> Result<()> {
    let branch = match &request.branch {
        Branch::Detached => return Ok(()),
        Branch::New(name) | Branch::Existing(name) => name,
    };
    let checked = repository
        .capture(repository.command()?.args(["check-ref-format", "--branch"]).arg(branch))?;
    let literal =
        branch.to_str().ok_or(Error::InvalidRequest { reason: RequestIssue::InvalidBranch })?;
    if checked != format!("{literal}\n").as_bytes() {
        return Err(Error::InvalidRequest { reason: RequestIssue::InvalidBranch });
    }
    match (&request.branch, repository.has_branch(branch)?) {
        (Branch::New(_), true) => return Err(Error::BranchExists { name: branch.clone() }),
        (Branch::Existing(_), false) => return Err(Error::BranchMissing { name: branch.clone() }),
        _ => {}
    }
    if matches!(request.branch, Branch::Existing(_))
        && repository.resolve(&qualified(branch))? != snapshot.commit
    {
        return Err(Error::HeadMismatch);
    }
    Ok(())
}

fn qualified(branch: &OsStr) -> OsString {
    let mut reference = OsString::from("refs/heads/");
    reference.push(branch);
    reference
}

struct Creation<'a> {
    repository: &'a Git,
    request: &'a AddRequest,
    snapshot: Snapshot,
    target: PathBuf,
    created: Vec<PathBuf>,
    branch_owned: bool,
    children: Vec<crate::submodules::Child>,
    #[cfg(test)]
    hook: Option<&'a dyn Fn(Phase) -> Result<()>>,
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    BeforeRegister,
    BranchCreated,
    BeforeCopy,
    BeforeRemove,
    BeforeDeleteRef,
}

impl<'a> Creation<'a> {
    fn new(repository: &'a Git, request: &'a AddRequest) -> Result<Self> {
        let (target, snapshot) = prepare(repository, request)?;
        let children = crate::submodules::plan(
            repository,
            &snapshot,
            request.submodules,
            request.source_mode,
        )?;
        Ok(Self {
            repository,
            request,
            snapshot,
            target,
            created: Vec::new(),
            branch_owned: false,
            children,
            #[cfg(test)]
            hook: None,
        })
    }

    fn execute(mut self) -> Result<Worktree> {
        match self.run() {
            Ok(tree) => Ok(tree),
            Err(original) => match self.rollback() {
                Ok(()) => Err(original),
                Err(cleanup) => Err(Error::Cleanup {
                    path: self.target,
                    branch: self.request.branch.clone(),
                    original: Box::new(original),
                    cleanup: Box::new(cleanup),
                }),
            },
        }
    }

    #[cfg(test)]
    fn checkpoint(&self, phase: Phase) -> Result<()> {
        if let Some(hook) = self.hook {
            hook(phase)?;
        }
        Ok(())
    }

    fn run(&mut self) -> Result<Worktree> {
        self.request.cancellation.check()?;
        self.reserve()?;
        self.request.cancellation.check()?;
        #[cfg(test)]
        self.checkpoint(Phase::BeforeRegister)?;
        self.register()?;
        #[cfg(test)]
        self.checkpoint(Phase::BeforeCopy)?;
        self.populate()?;
        let target = self.repository.select(&self.target);
        let mut tree = self.verify()?;
        crate::submodule_record::write(
            &target,
            self.request.submodules,
            &self.snapshot.submodules,
        )?;
        self.request.cancellation.check()?;
        if self.request.lock == Lock::Release {
            self.repository.capture(
                self.repository
                    .command()?
                    .args(["worktree", "unlock"])
                    .arg(dunce::simplified(&self.target)),
            )?;
            tree.locked = false;
            tree.reason = None;
        }
        Ok(tree)
    }

    fn populate(&self) -> Result<()> {
        match self.request.source_mode {
            SourceMode::Checkout => populate_tracked(
                &self.repository.root,
                &self.target,
                &self.snapshot.entries,
                &self.request.cancellation,
            )?,
            SourceMode::Committed => {
                crate::populate::create_parents(
                    &self.target,
                    crate::populate::parents(&self.snapshot.entries)?,
                    &self.request.cancellation,
                )?;
                crate::submodules::directories(&self.target, &self.snapshot.submodules)?;
                self.repository.select(&self.target).checkout(&self.snapshot.commit)?;
            }
        }
        if self.request.source_mode == SourceMode::Checkout {
            crate::submodules::directories(&self.target, &self.snapshot.submodules)?;
        }
        let target = self.repository.select(&self.target);
        crate::submodules::populate(
            &target,
            &self.children,
            self.request.source_mode,
            &self.request.cancellation,
        )?;
        crate::submodule_record::activate(&target, &self.children)?;
        Ok(())
    }

    fn reserve(&mut self) -> Result<()> {
        let mut missing = vec![self.target.clone()];
        let mut parent = self
            .target
            .parent()
            .ok_or(Error::InvalidRequest { reason: RequestIssue::InvalidDestination })?;
        while !parent.exists() {
            missing.push(parent.to_path_buf());
            parent = parent
                .parent()
                .ok_or(Error::InvalidRequest { reason: RequestIssue::InvalidDestination })?;
        }
        if !crate::platform::same_volume(&self.repository.root, parent)
            .map_err(|error| Error::io(parent, error))?
        {
            return Err(Error::Native(crate::Error::io(
                crate::Operation::Clone,
                &self.target,
                std::io::ErrorKind::CrossesDevices.into(),
            )));
        }
        let report = inspect_path(parent)?;
        if !report.supported() {
            return Err(Error::Native(crate::Error::UnsupportedPlatform));
        }
        for directory in missing.into_iter().rev() {
            fs::create_dir(&directory).map_err(|error| Error::io(&directory, error))?;
            self.created.push(directory);
        }
        Ok(())
    }

    fn register(&mut self) -> Result<()> {
        if let Branch::New(branch) = &self.request.branch {
            let absent = "0".repeat(self.snapshot.commit.len());
            self.repository.capture(
                self.repository
                    .command()?
                    .arg("update-ref")
                    .arg(qualified(branch))
                    .arg(&self.snapshot.commit)
                    .arg(absent),
            )?;
            self.branch_owned = true;
            #[cfg(test)]
            self.checkpoint(Phase::BranchCreated)?;
        }
        let mut command = self.repository.command()?;
        command.args(["worktree", "add", "--no-checkout", "--lock"]);
        if let Lock::Retain { reason: Some(reason) } = &self.request.lock {
            command.arg("--reason").arg(reason);
        }
        match &self.request.branch {
            Branch::Detached => {
                command
                    .args(["--detach", "--"])
                    .arg(dunce::simplified(&self.target))
                    .arg(&self.snapshot.commit);
            }
            Branch::New(branch) | Branch::Existing(branch) => {
                command.arg("--").arg(dunce::simplified(&self.target)).arg(branch);
            }
        }
        self.repository.capture(&mut command)?;
        Ok(())
    }

    fn verify(&self) -> Result<Worktree> {
        let target = self.repository.select(&self.target);
        match self.request.source_mode {
            SourceMode::Checkout => {
                target.read_tree(Some(&self.snapshot.commit))?;
                if self.repository.head()? != self.snapshot.commit {
                    return Err(Error::HeadMismatch);
                }
            }
            SourceMode::Committed => target.require_full_checkout()?,
        }
        let tree = self
            .repository
            .find_worktree(&self.target)?
            .ok_or_else(|| Error::WorktreeMissing { path: self.target.clone() })?;
        if tree.head.as_deref() != Some(self.snapshot.commit.as_str()) {
            return Err(Error::HeadMismatch);
        }
        if self.request.source_mode == SourceMode::Committed {
            target.require_visible_index()?;
            if !target.status()?.is_empty() {
                return Err(Error::DirtySource);
            }
            // Hooks may leave false-clean stat records.
            target.read_tree(None)?;
            target.read_tree(Some(&self.snapshot.commit))?;
        }
        if !target.status()?.is_empty() {
            return Err(Error::DirtySource);
        }
        Ok(tree)
    }

    fn rollback(&mut self) -> Result<()> {
        if self.created.last() == Some(&self.target) {
            if self.repository.find_worktree(&self.target)?.is_some() {
                #[cfg(test)]
                self.checkpoint(Phase::BeforeRemove)?;
                self.repository.capture(
                    self.repository
                        .command()?
                        .args(["worktree", "remove", "--force", "--force"])
                        .arg(dunce::simplified(&self.target)),
                )?;
            } else {
                match fs::remove_dir(&self.target) {
                    Ok(()) => {}
                    // Git can remove the reserved directory after registration fails.
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(Error::io(&self.target, error)),
                }
            }
            self.created.pop();
        }
        if self.branch_owned {
            if let Branch::New(branch) = &self.request.branch {
                #[cfg(test)]
                self.checkpoint(Phase::BeforeDeleteRef)?;
                self.repository.capture(
                    self.repository
                        .command()?
                        .args(["update-ref", "-d"])
                        .arg(qualified(branch))
                        .arg(&self.snapshot.commit),
                )?;
            }
        }
        for directory in self.created.iter().rev() {
            fs::remove_dir(directory).map_err(|error| Error::io(directory, error))?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "creation_tests.rs"]
mod tests;
