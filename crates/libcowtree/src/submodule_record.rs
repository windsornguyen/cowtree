// Copyright (c) 2026 Windsor Nguyen

//! Keep policy and child ownership in worktree-private metadata.

use crate::{
    SubmodulePolicy, WorktreeError as Error, WorktreeResult as Result,
    git::Git,
    git_tree::Pin,
    submodules::{Child, unavailable},
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Record {
    /// Exact standalone worktree owning this record.
    pub path: PathBuf,
    /// Policy selected when the worktree was created.
    pub policy: SubmodulePolicy,
    /// Child commit identities that clean removal must retain.
    pub pins: Vec<Pin>,
}

fn directory(repository: &Git) -> Result<PathBuf> {
    Ok(PathBuf::from(repository.text(repository.command()?.args([
        "rev-parse",
        "--path-format=absolute",
        "--absolute-git-dir",
    ]))?))
}

pub(crate) fn write(repository: &Git, policy: SubmodulePolicy, pins: &[Pin]) -> Result<()> {
    if pins.is_empty() && policy == SubmodulePolicy::default() {
        return Ok(());
    }
    let path = directory(repository)?.join("cowtree-submodules.json");
    let file = fs::File::options()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| Error::io(&path, e))?;
    serde_json::to_writer(
        file,
        &Record { path: repository.root.clone(), policy, pins: pins.into() },
    )
    .map_err(|source| Error::SubmoduleRecord { path: path.clone(), source })?;
    Ok(())
}

pub(crate) fn read(repository: &Git) -> Result<Option<Record>> {
    let path = directory(repository)?.join("cowtree-submodules.json");
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(Error::io(&path, error)),
    };
    let record: Record = serde_json::from_slice(&bytes)
        .map_err(|source| Error::SubmoduleRecord { path: path.clone(), source })?;
    if !same_file::is_same_file(&record.path, &repository.root).map_err(|e| Error::io(&path, e))? {
        return Err(unavailable(&path, "worktree policy belongs to another directory"));
    }
    Ok(Some(record))
}

/// Return the recorded standalone policy, or the default for an unmanaged directory.
pub fn submodule_policy(path: &Path) -> Result<SubmodulePolicy> {
    let selected = Git::at(path);
    let output = selected
        .command()?
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|e| Error::io(path, e))?;
    if output.status.code() == Some(128) {
        return Ok(SubmodulePolicy::default());
    }
    cowtree_git::checked(output)?;
    let repository = Git::discover(Some(path))?;
    Ok(read(&repository)?.map_or(SubmodulePolicy::default(), |record| record.policy))
}

/// Use Git's per-worktree configuration for child activation.
pub(crate) fn activate(repository: &Git, children: &[Child]) -> Result<()> {
    if children.is_empty() {
        return Ok(());
    }
    let common = repository.directory()?;
    let extension = repository
        .command()?
        .args(["config", "--bool", "extensions.worktreeConfig"])
        .output()
        .map_err(|e| Error::io(&common, e))?;
    if !matches!(extension.status.code(), Some(0 | 1)) {
        cowtree_git::checked(extension)?;
    } else if extension.stdout != b"true\n" {
        enable(repository, &common)?;
    }
    for child in children {
        repository.capture(
            repository
                .command()?
                .args(["config", "--worktree"])
                .arg(format!("submodule.{}.active", child.name))
                .arg("true"),
        )?;
    }
    Ok(())
}

fn enable(repository: &Git, common: &Path) -> Result<()> {
    // Do not activate previously ignored per-worktree settings.
    let mut directories = vec![common.to_path_buf()];
    let registrations = common.join("worktrees");
    if registrations.exists() {
        for entry in fs::read_dir(&registrations).map_err(|e| Error::io(&registrations, e))? {
            directories.push(entry.map_err(|e| Error::io(&registrations, e))?.path());
        }
    }
    if directories.iter().any(|path| path.join("config.worktree").exists()) {
        return Err(unavailable(
            common,
            "enable extensions.worktreeConfig explicitly before using existing config.worktree files",
        ));
    }
    let special = repository
        .command()?
        .args(["config", "--local", "--get", "core.worktree"])
        .output()
        .map_err(|e| Error::io(common, e))?;
    if special.status.success() {
        return Err(unavailable(
            common,
            "core.worktree requires explicit worktree-config migration",
        ));
    }
    if special.status.code() != Some(1) {
        cowtree_git::checked(special)?;
    }
    let bare = repository
        .command()?
        .args(["config", "--local", "--bool", "core.bare"])
        .output()
        .map_err(|e| Error::io(common, e))?;
    if bare.stdout == b"true\n" {
        return Err(unavailable(
            common,
            "bare repositories require explicit worktree-config migration",
        ));
    }
    if !matches!(bare.status.code(), Some(0 | 1)) {
        cowtree_git::checked(bare)?;
    }
    repository.capture(repository.command()?.args([
        "config",
        "--local",
        "extensions.worktreeConfig",
        "true",
    ]))?;
    Ok(())
}

pub(crate) fn verify_children(repository: &Git, pins: &[Pin], force: bool) -> Result<()> {
    for pin in pins {
        let path = crate::submodules::local(&repository.root, &pin.path)?;
        let metadata = path.join(".git");
        if !fs::symlink_metadata(&metadata).map_err(|e| Error::io(&metadata, e))?.is_dir() {
            return Err(unavailable(&path, "child Git directory is no longer private"));
        }
        let child = Git::at(&path);
        let checkout = Git::discover(Some(&path))?;
        if !same_file::is_same_file(&checkout.root, &path).map_err(|e| Error::io(&path, e))? {
            return Err(unavailable(&path, "child worktree authority has changed"));
        }
        let admin = directory(&child)?.canonicalize().map_err(|e| Error::io(&metadata, e))?;
        if admin != metadata.canonicalize().map_err(|e| Error::io(&metadata, e))? {
            return Err(unavailable(&path, "child Git authority has changed"));
        }
        let tree = child.tree(child.head()?)?;
        verify_children(&child, &tree.submodules, force)?;
        if !force {
            child.require_visible_index()?;
        }
        if !force
            && (child.head()? != pin.commit
                || !child
                    .capture(child.command()?.args([
                        "status",
                        "--porcelain=v1",
                        "--untracked-files=all",
                        "--ignore-submodules=none",
                    ]))?
                    .is_empty())
        {
            return Err(Error::DirtySource);
        }
    }
    Ok(())
}
