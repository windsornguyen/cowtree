// Copyright (c) 2026 Windsor Nguyen

//! Standalone creation owns its directory reservation and any newly created ref.
//!
//! Hold the common repository lock, pin a clean source, reserve an absent target,
//! register it with Git, and populate through the native batch API. Return only
//! after Git verifies the target against the pinned commit. Failure retires the
//! owned registration before conditionally deleting the ref and empty parents.

use crate::{
    AddRequest, Branch, Lock, SourceMode, Worktree, WorktreeError as Error,
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
    _lock: fs::File,
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
        if self.request.source_mode == SourceMode::Committed {
            return committed(&self.repository, &self.request);
        }
        create(&self.repository, &self.request)
    }
}

fn validate(request: &AddRequest) -> Result<()> {
    if request.path.as_os_str().is_empty() || request.revision.is_empty() {
        return Err(Error::InvalidRequest { reason: "path and revision must be nonempty" });
    }
    if let Lock::Retain { reason: Some(reason) } = &request.lock {
        if reason.is_empty() || reason.contains('\0') {
            return Err(Error::InvalidRequest {
                reason: "lock reason must be nonempty and contain no NUL",
            });
        }
    }
    if matches!(request.branch, Branch::Existing(_))
        && request.source_mode == SourceMode::Committed
        && request.revision != "HEAD"
    {
        return Err(Error::InvalidRequest {
            reason: "existing branch selects the committed revision",
        });
    }
    Ok(())
}

fn create(repository: &Git, request: &AddRequest) -> Result<Worktree> {
    Creation::new(repository, request)?.execute()
}

fn prepare(repository: &Git, request: &AddRequest) -> Result<(PathBuf, Snapshot)> {
    let target = resolve_target(&request.path)?;
    let snapshot = repository.snapshot()?;
    if repository.resolve(&request.revision)? != snapshot.commit {
        return Err(Error::HeadMismatch);
    }
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
                .ok_or(Error::InvalidRequest { reason: "invalid destination component" })?,
        );
        ancestor = ancestor
            .parent()
            .ok_or(Error::InvalidRequest { reason: "destination has no existing ancestor" })?;
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
        .capture(repository.command().args(["check-ref-format", "--branch"]).arg(branch))?;
    let literal =
        branch.to_str().ok_or(Error::InvalidRequest { reason: "branch must be UTF-8" })?;
    if checked != format!("{literal}\n").as_bytes() {
        return Err(Error::InvalidRequest { reason: "branch must be a literal local name" });
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
        Ok(Self {
            repository,
            request,
            snapshot,
            target,
            created: Vec::new(),
            branch_owned: false,
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
        populate_tracked(
            &self.repository.root,
            &self.target,
            &self.snapshot.entries,
            &self.request.cancellation,
        )?;
        self.verify()?;
        self.request.cancellation.check()?;
        if self.request.lock == Lock::Release {
            self.repository.capture(
                self.repository.command().args(["worktree", "unlock"]).arg(&self.target),
            )?;
        }
        self.repository
            .find_worktree(&self.target)?
            .ok_or_else(|| Error::WorktreeMissing { path: self.target.clone() })
    }

    fn reserve(&mut self) -> Result<()> {
        let mut missing = vec![self.target.clone()];
        let mut parent = self
            .target
            .parent()
            .ok_or(Error::InvalidRequest { reason: "destination requires a parent" })?;
        while !parent.exists() {
            missing.push(parent.to_path_buf());
            parent = parent.parent().ok_or(Error::InvalidRequest {
                reason: "destination requires an existing ancestor",
            })?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let source = fs::metadata(&self.repository.root)
                .map_err(|error| Error::io(&self.repository.root, error))?;
            let destination = fs::metadata(parent).map_err(|error| Error::io(parent, error))?;
            if source.dev() != destination.dev() {
                return Err(Error::Native(crate::Error::io(
                    crate::Operation::Clone,
                    &self.target,
                    std::io::ErrorKind::CrossesDevices.into(),
                )));
            }
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
                    .command()
                    .arg("update-ref")
                    .arg(qualified(branch))
                    .arg(&self.snapshot.commit)
                    .arg(absent),
            )?;
            self.branch_owned = true;
            #[cfg(test)]
            self.checkpoint(Phase::BranchCreated)?;
        }
        let mut command = self.repository.command();
        command.args(["worktree", "add", "--no-checkout", "--lock"]);
        if let Lock::Retain { reason: Some(reason) } = &self.request.lock {
            command.arg("--reason").arg(reason);
        }
        match &self.request.branch {
            Branch::Detached => {
                command.args(["--detach", "--"]).arg(&self.target).arg(&self.snapshot.commit);
            }
            Branch::New(branch) | Branch::Existing(branch) => {
                command.arg("--").arg(&self.target).arg(branch);
            }
        }
        self.repository.capture(&mut command)?;
        Ok(())
    }

    fn verify(&self) -> Result<()> {
        let target = Git { root: self.target.clone() };
        target.capture(
            target
                .command()
                .args(["-c", "core.fsmonitor=false", "-c", "core.ignorestat=false", "read-tree"])
                .arg(&self.snapshot.commit),
        )?;
        if self.repository.head()? != self.snapshot.commit || target.head()? != self.snapshot.commit
        {
            return Err(Error::HeadMismatch);
        }
        if !target.status()?.is_empty() {
            return Err(Error::DirtySource);
        }
        Ok(())
    }

    fn rollback(&mut self) -> Result<()> {
        if self.created.last() == Some(&self.target) {
            if self.repository.find_worktree(&self.target)?.is_some() {
                #[cfg(test)]
                self.checkpoint(Phase::BeforeRemove)?;
                self.repository.capture(
                    self.repository
                        .command()
                        .args(["worktree", "remove", "--force", "--force"])
                        .arg(&self.target),
                )?;
            } else {
                fs::remove_dir(&self.target).map_err(|error| Error::io(&self.target, error))?;
            }
            self.created.pop();
        }
        if self.branch_owned {
            if let Branch::New(branch) = &self.request.branch {
                #[cfg(test)]
                self.checkpoint(Phase::BeforeDeleteRef)?;
                self.repository.capture(
                    self.repository
                        .command()
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

fn committed(repository: &Git, request: &AddRequest) -> Result<Worktree> {
    let revision = match &request.branch {
        Branch::Existing(branch) => qualified(branch),
        _ => request.revision.clone(),
    };
    let commit = repository.resolve(&revision)?;
    repository.entries(OsStr::new(&commit))?;
    let parent = repository
        .root
        .parent()
        .ok_or(Error::InvalidRequest { reason: "source requires a parent" })?;
    let directory = tempfile::Builder::new()
        .prefix(".cowtree-seed-")
        .tempdir_in(parent)
        .map_err(|error| Error::io(parent, error))?;
    let root = directory.keep();
    let source = root.join("tree");
    let registered = repository.capture(
        repository
            .command()
            .args(["worktree", "add", "--detach", "--lock", "--"])
            .arg(&source)
            .arg(&commit),
    );
    let result = registered.and_then(|_| {
        let mut pinned = request.clone();
        pinned.path =
            std::path::absolute(&request.path).map_err(|error| Error::io(&request.path, error))?;
        pinned.revision = commit.into();
        create(&Git { root: source.clone() }, &pinned)
    });
    let cleanup = remove_seed(repository, &root, &source);
    match (result, cleanup) {
        (result, Ok(())) => result,
        (Ok(_), Err(source)) => Err(Error::SeedCleanup { path: root, source: Box::new(source) }),
        (Err(original), Err(cleanup)) => Err(Error::Cleanup {
            path: root,
            branch: request.branch.clone(),
            original: Box::new(original),
            cleanup: Box::new(cleanup),
        }),
    }
}

#[cfg(test)]
#[path = "creation_tests.rs"]
mod tests;

fn remove_seed(repository: &Git, root: &Path, source: &Path) -> Result<()> {
    if repository.find_worktree(source)?.is_some() {
        repository.capture(
            repository
                .command()
                .args(["worktree", "remove", "--force", "--force", "--"])
                .arg(source),
        )?;
    }
    fs::remove_dir_all(root).map_err(|error| Error::io(root, error))
}
