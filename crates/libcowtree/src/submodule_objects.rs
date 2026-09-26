// Copyright (c) 2026 Windsor Nguyen

//! Clone object data without sharing mutable repository metadata.

use crate::{
    Cancellation, WorktreeError as Error, WorktreeResult as Result, git::Git,
    submodules::unavailable,
};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn objects(source: &Git) -> Result<PathBuf> {
    Ok(PathBuf::from(source.text(source.command()?.args([
        "rev-parse",
        "--path-format=absolute",
        "--git-path",
        "objects",
    ]))?))
}

pub(crate) fn check(source: &Git) -> Result<()> {
    let partial = source.output(source.command()?.args([
        "config",
        "--get-regexp",
        r"^(extensions\.partialclone|remote\..*\.promisor)$",
    ]))?;
    if partial.status.success() {
        return Err(unavailable(
            &source.root,
            "configured promisor repositories require local hydration first",
        ));
    }
    if partial.status.code() != Some(1) {
        cowtree_git::checked(partial)?;
    }
    if source.text(source.command()?.args(["rev-parse", "--is-shallow-repository"]))? == "true" {
        return Err(unavailable(&source.root, "shallow submodule history is unsupported"));
    }
    let root = objects(source)?;
    for name in ["info/alternates", "info/http-alternates"] {
        let path = root.join(name);
        match fs::read(&path) {
            Ok(bytes) if !bytes.is_empty() => {
                return Err(unavailable(&root, "external object alternates are unsupported"));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(Error::io(&path, error)),
        }
    }
    Ok(())
}

pub(crate) fn create(
    source: &Git,
    target: &Path,
    commit: &str,
    cancellation: &Cancellation,
) -> Result<Git> {
    if fs::read_dir(target).map_err(|e| Error::io(target, e))?.next().is_some() {
        return Err(unavailable(target, "child destination contains unexpected files"));
    }
    let format = source.text(source.command()?.args(["rev-parse", "--show-object-format"]))?;
    let repository = Git::at(target);
    repository.capture(
        repository
            .command()?
            .args(["init", "--quiet", "--template="])
            .arg(format!("--object-format={format}")),
    )?;
    copy(&objects(source)?, &target.join(".git/objects"), cancellation)?;
    repository.capture(repository.command()?.args(["update-ref", "--no-deref", "HEAD", commit]))?;
    repository.capture(repository.command()?.args(["config", "core.logAllRefUpdates", "true"]))?;
    Ok(repository)
}

fn copy(source: &Path, target: &Path, cancellation: &Cancellation) -> Result<()> {
    let mut pending = vec![PathBuf::new()];
    while let Some(relative) = pending.pop() {
        let directory = source.join(&relative);
        for entry in fs::read_dir(&directory).map_err(|e| Error::io(&directory, e))? {
            cancellation.check()?;
            let entry = entry.map_err(|e| Error::io(&directory, e))?;
            let relative = relative.join(entry.file_name());
            let destination = target.join(&relative);
            let kind = entry.file_type().map_err(|e| Error::io(&entry.path(), e))?;
            if kind.is_dir() {
                fs::create_dir_all(&destination).map_err(|e| Error::io(&destination, e))?;
                pending.push(relative);
            } else if kind.is_file() {
                crate::clone_file(&entry.path(), &destination)?;
            } else {
                return Err(unavailable(
                    &entry.path(),
                    "object storage contains a link or special file",
                ));
            }
        }
    }
    Ok(())
}
