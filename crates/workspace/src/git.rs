// Copyright (c) 2026 Windsor Nguyen

//! Bind managed Git operations to one authority.

use crate::{CommitId, Error, Result, error::Issue};
use cowtree_git::{Client, checked};
use std::{
    fs::File,
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone)]
pub(crate) struct Repository {
    client: Client,
    /// Common directory persisted by the workspace.
    pub directory: PathBuf,
}

impl Deref for Repository {
    type Target = Client;
    fn deref(&self) -> &Client {
        &self.client
    }
}
impl DerefMut for Repository {
    fn deref_mut(&mut self) -> &mut Client {
        &mut self.client
    }
}

impl Repository {
    pub fn discover(source: &Path) -> Result<Self> {
        let client = Client::discover(Some(source))?;
        if client.root.to_str().is_none() {
            return Err(Issue::NonUtf8.at(&client.root));
        }
        let directory = client.directory()?;
        Ok(Self { client, directory })
    }

    pub fn authority(directory: &Path, locks: Vec<Arc<File>>) -> Self {
        Self { client: Client { root: directory.into(), locks }, directory: directory.into() }
    }

    pub fn select(&self, root: &Path) -> Self {
        Self { client: self.client.select(root), directory: self.directory.clone() }
    }

    pub fn locked(&self) -> Result<Self> {
        Ok(Self {
            client: self.client.locked_at(&self.directory)?,
            directory: self.directory.clone(),
        })
    }

    pub fn head(&self) -> Result<CommitId> {
        CommitId::parse(self.client.head()?)
    }

    pub fn detached(&self, expected: &[CommitId]) -> Result<()> {
        let output = self.output(self.command()?.args(["symbolic-ref", "--quiet", "HEAD"]))?;
        if output.status.success() {
            return Err(Issue::NotDetached.at(&self.root));
        }
        if output.status.code() != Some(1) {
            checked(output)?;
        }
        if !expected.contains(&self.head()?) {
            return Err(Issue::HeadChanged.at(&self.root));
        }
        Ok(())
    }

    pub fn reset_index(&self, commit: &CommitId) -> Result<()> {
        self.capture(self.command()?.args([
            "-c",
            "core.fsync=all",
            "reset",
            "--mixed",
            "-q",
            commit.as_str(),
        ]))?;
        Ok(())
    }
}

impl Repository {
    pub(crate) fn worktrees(&self) -> Result<Vec<PathBuf>> {
        use std::os::unix::ffi::OsStringExt;
        let bytes =
            self.capture(self.command()?.args(["worktree", "list", "--porcelain", "-z"]))?;
        Ok(bytes
            .split(|byte| *byte == 0)
            .filter_map(|field| field.strip_prefix(b"worktree "))
            .map(|path| PathBuf::from(std::ffi::OsString::from_vec(path.into())))
            .collect())
    }

    pub(crate) fn has_worktree(&self, path: &Path) -> Result<bool> {
        use std::os::unix::fs::MetadataExt;
        for tree in self.worktrees()? {
            if tree == path {
                return Ok(true);
            }
            match (std::fs::metadata(&tree), std::fs::metadata(path)) {
                (Ok(left), Ok(right)) if left.dev() == right.dev() && left.ino() == right.ino() => {
                    return Ok(true);
                }
                (Err(error), _) if error.kind() != std::io::ErrorKind::NotFound => {
                    return Err(Error::io(&tree, error));
                }
                (_, Err(error)) if error.kind() != std::io::ErrorKind::NotFound => {
                    return Err(Error::io(path, error));
                }
                _ => {}
            }
        }
        Ok(false)
    }
}
