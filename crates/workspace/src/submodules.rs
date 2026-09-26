// Copyright (c) 2026 Windsor Nguyen

//! Admit initialized pinned dependencies and verify their copied bytes with fresh indexes.

use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{
    CommitId, Error, Policy, Result, SubmodulePolicy, error::Issue, git::Repository, paths,
    records, types::Pin,
};

const MARKER: &str = ".cowtree-pin";

impl Repository {
    pub(crate) fn snapshot(&self, policy: SubmodulePolicy) -> Result<(CommitId, Vec<Pin>)> {
        let sparse =
            self.output(self.command()?.args(["config", "--bool", "core.sparseCheckout"]))?;
        if sparse.stdout == b"true\n" {
            return Err(Issue::SparseCheckout.at(&self.root));
        }
        if !matches!(sparse.status.code(), Some(0 | 1)) {
            return Err(cowtree_git::Error::Command {
                status: sparse.status,
                stderr: String::from_utf8_lossy(&sparse.stderr).into_owned(),
            }
            .into());
        }
        let commit = self.head()?;
        let tree = self.capture(self.command()?.args(["ls-tree", "-r", "-z", commit.as_str()]))?;
        let mut pins = Vec::new();
        for record in tree.split(|byte| *byte == 0).filter(|record| !record.is_empty()) {
            let record = paths::utf8(record, &self.root)?;
            let (header, path) =
                record.split_once('\t').ok_or_else(|| Issue::IncompleteRecord.at(&self.root))?;
            let fields: Vec<_> = header.split(' ').collect();
            if fields.len() != 3 {
                return Err(Issue::IncompleteRecord.at(&self.root));
            }
            if fields[0] == "160000" {
                if policy == SubmodulePolicy::Reject {
                    return Err(Issue::InvalidSubmodule.at(&self.root.join(path)));
                }
                pins.push(Pin { path: path.into(), commit: CommitId::parse(fields[2].into())? });
            } else if !matches!(fields[0], "100644" | "100755" | "120000") {
                return Err(Issue::UnsupportedEntry.at(&self.root.join(path)));
            }
        }
        let flags = self.capture(self.command()?.args(["ls-files", "-v", "-z"]))?;
        if flags.split(|byte| *byte == 0).any(|record| {
            record.first().is_some_and(|flag| flag.is_ascii_lowercase() || *flag == b'S')
        }) || !self.status()?.is_empty()
        {
            return Err(Issue::DirtyCheckout.at(&self.root));
        }
        Ok((commit, pins))
    }

    pub(crate) fn verify_tree(&self, tree: &Path, commit: &CommitId) -> Result<()> {
        let git_directory =
            self.text(self.command()?.args(["rev-parse", "--path-format=absolute", "--git-dir"]))?;
        let temporary = tempfile::Builder::new()
            .prefix("cowtree-pin-index-")
            .tempdir()
            .map_err(|error| Error::io(tree, error))?;
        let command = || -> Result<std::process::Command> {
            let mut command = self.command()?;
            command
                .args(["--git-dir", &git_directory, "--work-tree"])
                .arg(tree)
                .args([
                    "-c",
                    "core.filemode=true",
                    "-c",
                    "core.fsmonitor=false",
                    "-c",
                    "core.ignorestat=false",
                ])
                .env("GIT_INDEX_FILE", temporary.path().join("index"));
            Ok(command)
        };
        self.capture(command()?.args(["read-tree", commit.as_str()]))?;
        let changed = self.capture(command()?.args([
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=no",
        ]))?;
        if !changed.is_empty() {
            return Err(Issue::DirtyCheckout.at(tree));
        }
        Ok(())
    }
}

pub(crate) fn admit(repository: &Repository, policy: &Policy) -> Result<Policy> {
    if !policy.pins.is_empty() {
        return Err(Issue::InvalidSubmodule.at(&repository.root));
    }
    let (_, pins) = repository.snapshot(policy.submodules)?;
    for pin in &pins {
        let path = paths::local(&repository.root, &pin.path)?;
        let metadata = fs::symlink_metadata(&path).map_err(|error| Error::io(&path, error))?;
        if !metadata.is_dir() {
            return Err(Issue::InvalidSubmodule.at(&path));
        }
        let child = Repository::discover(&path)?;
        if child.root != path {
            return Err(Issue::InvalidSubmodule.at(&path));
        }
        let (commit, _) = child.snapshot(SubmodulePolicy::Reject)?;
        if commit != pin.commit {
            return Err(Issue::HeadChanged.at(&path));
        }
        validate_links(&child)?;
        child.verify_tree(&path, &pin.commit)?;
        if !child
            .capture(child.command()?.args(["ls-files", "--others", "--directory", "-z"]))?
            .is_empty()
        {
            return Err(Issue::DirtyCheckout.at(&path));
        }
        if paths::exists(&path.join(MARKER))? {
            return Err(Issue::InvalidSubmodule.at(&path));
        }
        for prefix in policy.derived.iter().chain(&policy.ephemeral).chain(&policy.ignored) {
            if overlaps(&pin.path, prefix) {
                return Err(Issue::ConflictingPolicy.at(&path));
            }
        }
    }
    let mut admitted = policy.clone();
    admitted.pins = pins;
    Ok(admitted)
}

pub(crate) fn materialize(
    repository: &Repository,
    source: &Path,
    target: &Path,
    policy: &Policy,
) -> Result<()> {
    for pin in &policy.pins {
        let mut child = Repository::discover(&source.join(&pin.path))?;
        child.locks = repository.locks.clone();
        if child.head()? != pin.commit {
            return Err(Issue::HeadChanged.at(&child.root));
        }
        let copied = paths::local(target, &pin.path)?;
        child.verify_tree(&copied, &pin.commit)?;
        let marker = copied.join(MARKER);
        let file = fs::File::options()
            .write(true)
            .create_new(true)
            .open(&marker)
            .map_err(|error| Error::io(&marker, error))?;
        serde_json::to_writer(&file, pin).map_err(|error| Error::record(&marker, error))?;
        records::sync_file(&file).map_err(|error| Error::io(&marker, error))?;
    }
    Ok(())
}

fn overlaps(left: &str, right: &str) -> bool {
    left == right
        || left.strip_prefix(right).is_some_and(|tail| tail.starts_with('/'))
        || right.strip_prefix(left).is_some_and(|tail| tail.starts_with('/'))
}

fn validate_links(repository: &Repository) -> Result<()> {
    let tree = repository.capture(repository.command()?.args(["ls-tree", "-r", "-z", "HEAD"]))?;
    for record in tree.split(|byte| *byte == 0).filter(|record| record.starts_with(b"120000 ")) {
        let (_, name) = record.split_at(
            record
                .iter()
                .position(|byte| *byte == b'\t')
                .ok_or_else(|| Issue::IncompleteRecord.at(&repository.root))?
                + 1,
        );
        let path = paths::local(&repository.root, &paths::utf8(name, &repository.root)?)?;
        let link = fs::read_link(&path).map_err(|error| Error::io(&path, error))?;
        if link.is_absolute() {
            return Err(Issue::InvalidSubmodule.at(&path));
        }
        let mut logical = path.parent().ok_or_else(|| Issue::InvalidPath.at(&path))?.to_path_buf();
        for component in link.components() {
            match component {
                std::path::Component::ParentDir => {
                    logical.pop();
                }
                std::path::Component::CurDir => {}
                std::path::Component::Normal(part) => logical.push(part),
                _ => return Err(Issue::InvalidSubmodule.at(&path)),
            }
        }
        let resolved: PathBuf = resolve_links(&path)?;
        if !logical.starts_with(&repository.root) || !resolved.starts_with(&repository.root) {
            return Err(Issue::InvalidSubmodule.at(&path));
        }
    }
    Ok(())
}

/// Resolve links iteratively, allowing missing terminal names inside the dependency.
fn resolve_links(path: &Path) -> Result<PathBuf> {
    use std::{
        collections::{BTreeSet, VecDeque},
        ffi::OsString,
    };
    enum Step {
        Component(OsString),
        EndLink(PathBuf),
    }
    let mut pending: VecDeque<_> =
        path.components().map(|part| Step::Component(part.as_os_str().to_owned())).collect();
    let mut resolved = PathBuf::new();
    let mut active = BTreeSet::new();
    while let Some(step) = pending.pop_front() {
        let part = match step {
            Step::EndLink(path) => {
                active.remove(&path);
                continue;
            }
            Step::Component(part) => part,
        };
        if part == "/" {
            resolved = PathBuf::from("/");
            continue;
        }
        if part == "." {
            continue;
        }
        if part == ".." {
            resolved.pop();
            continue;
        }
        let candidate = resolved.join(&part);
        match fs::symlink_metadata(&candidate) {
            Ok(metadata) if metadata.is_symlink() => {
                if !active.insert(candidate.clone()) {
                    return Err(Issue::InvalidSubmodule.at(path));
                }
                let link =
                    fs::read_link(&candidate).map_err(|error| Error::io(&candidate, error))?;
                pending.push_front(Step::EndLink(candidate));
                for component in link.components().rev() {
                    pending.push_front(Step::Component(component.as_os_str().to_owned()));
                }
            }
            Ok(_) => resolved.push(part),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                resolved.push(part)
            }
            Err(error) => return Err(Error::io(&candidate, error)),
        }
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_resolution_distinguishes_cycles_from_repeated_noncyclic_paths() -> Result<()> {
        let root = tempfile::tempdir().map_err(|error| Error::io(Path::new("temporary"), error))?;
        let root = root.path().canonicalize().map_err(|error| Error::io(root.path(), error))?;
        std::os::unix::fs::symlink(".", root.join("link"))
            .map_err(|error| Error::io(&root, error))?;
        assert_eq!(resolve_links(&root.join("link/link/future"))?, root.join("future"));
        std::os::unix::fs::symlink("cycle", root.join("cycle"))
            .map_err(|error| Error::io(&root, error))?;
        assert!(matches!(
            resolve_links(&root.join("cycle")),
            Err(Error::State { issue: Issue::InvalidSubmodule, .. })
        ));
        Ok(())
    }
}
