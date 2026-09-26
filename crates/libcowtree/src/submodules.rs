// Copyright (c) 2026 Windsor Nguyen

//! Plan pinned children before claiming a destination.

use crate::{
    Cancellation, FileMode, SourceMode, SubmodulePolicy, TrackedFile, WorktreeError as Error,
    WorktreeResult as Result,
    git::Git,
    git_tree::{Pin, Snapshot},
};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

pub(crate) struct Child {
    /// Relative path and selected gitlink commit.
    pub pin: Pin,
    /// Logical name in the selected .gitmodules blob.
    pub name: String,
    /// Local repository containing the pinned objects.
    pub source: Git,
    /// Source working directory for checkout-mode cloning.
    pub checkout: PathBuf,
    /// Immutable tracked entries at the pin.
    pub snapshot: Snapshot,
    /// Recursively planned independent children.
    pub children: Vec<Child>,
}

pub(crate) fn plan(
    repository: &Git,
    snapshot: &Snapshot,
    policy: SubmodulePolicy,
    mode: SourceMode,
) -> Result<Vec<Child>> {
    if policy == SubmodulePolicy::Reject {
        if let Some(pin) = snapshot.submodules.first() {
            return Err(Error::Submodule { path: pin.path.clone() });
        }
        return Ok(Vec::new());
    }
    if policy == SubmodulePolicy::LeaveUninitialized {
        for pin in &snapshot.submodules {
            let path = local(&repository.root, &pin.path)?;
            match path.join(".git").symlink_metadata() {
                Ok(_) => return Err(Error::SubmoduleInitialized { path: pin.path.clone() }),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(Error::io(&path, error)),
            }
            if path.exists()
                && fs::read_dir(&path).map_err(|e| Error::io(&path, e))?.next().is_some()
            {
                return Err(unavailable(&pin.path, "uninitialized path contains files"));
            }
        }
        return Ok(Vec::new());
    }
    for key in [
        "GIT_DIR",
        "GIT_COMMON_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ] {
        if std::env::var_os(key).is_some() {
            return Err(unavailable(
                &repository.root,
                &format!("{key} cannot redirect private child repositories"),
            ));
        }
    }
    children(repository, &repository.root, snapshot, mode, 0)
}

fn children(
    repository: &Git,
    checkout: &Path,
    snapshot: &Snapshot,
    mode: SourceMode,
    depth: usize,
) -> Result<Vec<Child>> {
    if depth >= 32 {
        return Err(unavailable(checkout, "submodule nesting exceeds 32"));
    }
    if snapshot.submodules.is_empty() {
        return Ok(Vec::new());
    }
    let names = names(repository, &snapshot.commit)?;
    let mut result = Vec::new();
    for pin in &snapshot.submodules {
        let name = names
            .get(&pin.path)
            .ok_or_else(|| unavailable(&pin.path, "missing pinned .gitmodules entry"))?;
        let path = local(checkout, &pin.path)?;
        let source = source(repository, &path, name)?;
        crate::submodule_objects::check(&source)?;
        let selected = source.tree(pin.commit.clone()).map_err(|source| {
            Error::SubmoduleObjects { path: pin.path.clone(), source: Box::new(source) }
        })?;
        if mode == SourceMode::Checkout {
            let current = Git::discover(Some(&path))?.snapshot()?;
            if current.commit != pin.commit {
                return Err(Error::HeadMismatch);
            }
        }
        let children = children(&source, &path, &selected, mode, depth + 1)?;
        result.push(Child {
            pin: pin.clone(),
            name: name.clone(),
            source,
            checkout: path,
            snapshot: selected,
            children,
        });
    }
    Ok(result)
}

fn source(repository: &Git, checkout: &Path, name: &str) -> Result<Git> {
    if checkout.join(".git").exists() {
        let source = Git::discover(Some(checkout))?;
        if source.root != checkout.canonicalize().map_err(|e| Error::io(checkout, e))? {
            return Err(unavailable(checkout, "child repository identity differs"));
        }
        return Ok(source);
    }
    let storage = repository.text(
        repository
            .command()?
            .args(["rev-parse", "--path-format=absolute", "--git-path"])
            .arg(format!("modules/{name}")),
    )?;
    let storage = PathBuf::from(storage);
    if !storage.is_dir() {
        return Err(unavailable(checkout, "pinned objects are not available locally"));
    }
    Ok(Git::at(&storage))
}

fn names(repository: &Git, commit: &str) -> Result<BTreeMap<PathBuf, String>> {
    let data = repository.capture(repository.command()?.args([
        "config",
        "--null",
        "--blob",
        &format!("{commit}:.gitmodules"),
        "--get-regexp",
        r"^submodule\..*\.path$",
    ]))?;
    let mut names = BTreeMap::new();
    for record in data.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let record = std::str::from_utf8(record)
            .map_err(|_| unavailable(&repository.root, "submodule names require UTF-8"))?;
        let (key, path) = record
            .split_once('\n')
            .ok_or_else(|| unavailable(&repository.root, "invalid .gitmodules record"))?;
        let name = key
            .strip_prefix("submodule.")
            .and_then(|key| key.strip_suffix(".path"))
            .ok_or_else(|| unavailable(&repository.root, "invalid submodule name"))?;
        TrackedFile::new(PathBuf::from(name), FileMode::Regular)?;
        let path = TrackedFile::new(PathBuf::from(path), FileMode::Regular)?;
        if names.insert(path.path().into(), name.into()).is_some() {
            return Err(unavailable(path.path(), "duplicate .gitmodules path"));
        }
    }
    Ok(names)
}

pub(crate) fn local(root: &Path, relative: &Path) -> Result<PathBuf> {
    TrackedFile::new(relative.into(), FileMode::Regular)?;
    let mut path = root.to_path_buf();
    for component in relative.components() {
        path.push(component);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if !metadata.is_dir() => {
                return Err(unavailable(&path, "submodule path is not a directory"));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(Error::io(&path, error)),
        }
    }
    Ok(path)
}

pub(crate) fn directories(target: &Path, pins: &[Pin]) -> Result<()> {
    for pin in pins {
        let path = local(target, &pin.path)?;
        fs::create_dir_all(path.parent().ok_or_else(|| unavailable(&path, "missing parent"))?)
            .map_err(|e| Error::io(&path, e))?;
        fs::create_dir(&path).map_err(|e| Error::io(&path, e))?;
    }
    Ok(())
}

pub(crate) fn populate(
    parent: &Git,
    children: &[Child],
    mode: SourceMode,
    cancellation: &Cancellation,
) -> Result<()> {
    for child in children {
        cancellation.check()?;
        let target = local(&parent.root, &child.pin.path)?;
        let repository = crate::submodule_objects::create(
            &child.source,
            &target,
            &child.pin.commit,
            cancellation,
        )?;
        match mode {
            SourceMode::Checkout => crate::populate_tracked(
                &child.checkout,
                &target,
                &child.snapshot.entries,
                cancellation,
            )?,
            SourceMode::Committed => {
                crate::populate::create_parents(
                    &target,
                    crate::populate::parents(&child.snapshot.entries)?,
                    cancellation,
                )?;
                directories(&target, &child.snapshot.submodules)?;
                repository.checkout(&child.pin.commit)?;
            }
        }
        if mode == SourceMode::Checkout {
            directories(&target, &child.snapshot.submodules)?;
        }
        populate(&repository, &child.children, mode, cancellation)?;
        for nested in &child.children {
            repository.capture(
                repository
                    .command()?
                    .args(["config", "--local"])
                    .arg(format!("submodule.{}.active", nested.name))
                    .arg("true"),
            )?;
        }
        repository.require_visible_index()?;
        if mode == SourceMode::Committed && !repository.status()?.is_empty() {
            return Err(Error::DirtySource);
        }
        repository.read_tree(None)?;
        repository.read_tree(Some(&child.pin.commit))?;
        if repository.head()? != child.pin.commit || !repository.status()?.is_empty() {
            return Err(Error::DirtySource);
        }
    }
    Ok(())
}

pub(crate) fn unavailable(path: &Path, reason: &str) -> Error {
    Error::SubmoduleState { path: path.into(), reason: reason.into() }
}
