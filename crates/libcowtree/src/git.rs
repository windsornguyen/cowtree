// Copyright (c) 2026 Windsor Nguyen

//! Standalone admission and Git worktree records.

use crate::{GitField, Worktree, WorktreeError as Error, WorktreeResult as Result};
use cowtree_git::{Client, checked, decode_path};
use std::{
    ffi::OsStr,
    ops::Deref,
    path::{Path, PathBuf},
};

pub(crate) struct Git(Client);

pub(crate) use crate::git_tree::Snapshot;

impl Deref for Git {
    type Target = Client;
    fn deref(&self) -> &Client {
        &self.0
    }
}

impl Git {
    pub(crate) fn at(root: &Path) -> Self {
        Self(Client::at(root))
    }
    pub(crate) fn discover(source: Option<&Path>) -> Result<Self> {
        Ok(Self(Client::discover(source)?))
    }
    pub(crate) fn select(&self, root: &Path) -> Self {
        Self(self.0.select(root))
    }

    pub(crate) fn require_full_checkout(&self) -> Result<()> {
        let sparse = self
            .command()?
            .args(["config", "--bool", "core.sparseCheckout"])
            .output()
            .map_err(|error| Error::io(&self.root, error))?;
        if !matches!(sparse.status.code(), Some(0 | 1)) {
            checked(sparse)?;
        } else if sparse.stdout == b"true\n" {
            return Err(Error::SparseCheckout);
        }
        Ok(())
    }

    pub(crate) fn snapshot(&self) -> Result<Snapshot> {
        self.require_full_checkout()?;
        let commit = self.head()?;
        let snapshot = self.tree(commit)?;
        self.require_visible_index()?;
        if !self.status()?.is_empty() {
            return Err(Error::DirtySource);
        }
        Ok(snapshot)
    }

    pub(crate) fn require_visible_index(&self) -> Result<()> {
        let flags = self.capture(self.command()?.args(["ls-files", "-v", "-z"]))?;
        if flags.split(|byte| *byte == 0).any(|record| {
            record.first().is_some_and(|flag| flag.is_ascii_lowercase() || *flag == b'S')
        }) {
            return Err(Error::DirtySource);
        }
        Ok(())
    }

    /// Load pinned entries or clear the index.
    pub(crate) fn read_tree(&self, commit: Option<&str>) -> Result<()> {
        let mut command = self.command()?;
        command.args(["-c", "core.fsmonitor=false", "-c", "core.ignorestat=false", "read-tree"]);
        command.arg(commit.unwrap_or("--empty"));
        self.capture(&mut command)?;
        Ok(())
    }

    /// Materialize each tracked name without replacing aliases.
    pub(crate) fn checkout(&self, commit: &str) -> Result<()> {
        self.read_tree(Some(commit))?;
        self.capture(self.command()?.args([
            "-c",
            "core.symlinks=true",
            "checkout-index",
            "--all",
            "--index",
        ]))?;
        let absent = "0".repeat(commit.len());
        self.capture(self.command()?.args([
            "hook",
            "run",
            "--ignore-missing",
            "post-checkout",
            "--",
            &absent,
            commit,
            "1",
        ]))?;
        Ok(())
    }

    pub(crate) fn resolve(&self, revision: &OsStr) -> Result<String> {
        let mut selected = revision.to_os_string();
        selected.push("^{commit}");
        Ok(self.text(
            self.command()?.args(["rev-parse", "--verify", "--end-of-options"]).arg(selected),
        )?)
    }

    pub(crate) fn has_branch(&self, branch: &OsStr) -> Result<bool> {
        let mut name = std::ffi::OsString::from("refs/heads/");
        name.push(branch);
        let output = self
            .command()?
            .args(["show-ref", "--verify", "--quiet"])
            .arg(name)
            .output()
            .map_err(|error| Error::io(&self.root, error))?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Ok(checked(output).map(|_| true)?),
        }
    }

    pub(crate) fn worktrees(&self) -> Result<Vec<Worktree>> {
        let data = self.capture(self.command()?.args(["worktree", "list", "--porcelain", "-z"]))?;
        let mut records = Vec::new();
        let mut fields = Vec::new();
        for field in data.split(|byte| *byte == 0) {
            if field.is_empty() {
                if !fields.is_empty() {
                    records.push(parse_worktree(&fields)?);
                    fields.clear();
                }
            } else {
                fields.push(field);
            }
        }
        if !fields.is_empty() {
            return Err(Error::GitResponse { field: GitField::WorktreeTerminator });
        }
        Ok(records)
    }

    pub(crate) fn find_worktree(&self, path: &Path) -> Result<Option<Worktree>> {
        for tree in self.worktrees()? {
            if tree.path == path {
                return Ok(Some(tree));
            }
            match same_file::is_same_file(&tree.path, path) {
                Ok(true) => return Ok(Some(tree)),
                Ok(false) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(Error::io(path, error)),
            }
        }
        Ok(None)
    }
}

fn parse_worktree(fields: &[&[u8]]) -> Result<Worktree> {
    let mut path = None;
    let mut record = Worktree {
        path: PathBuf::new(),
        head: None,
        branch: None,
        detached: false,
        prunable: false,
        locked: false,
        reason: None,
    };
    for field in fields {
        let mut pair = field.splitn(2, |byte| *byte == b' ');
        let key = pair.next().ok_or(Error::GitResponse { field: GitField::WorktreeKey })?;
        let value = pair.next().unwrap_or_default();
        match key {
            b"worktree" => path = Some(decode_path(value.to_vec())?),
            b"HEAD" => {
                record.head = Some(
                    String::from_utf8(value.to_vec())
                        .map_err(|_| Error::GitResponse { field: GitField::Head })?,
                )
            }
            b"branch" => {
                record.branch = Some(
                    String::from_utf8(value.to_vec())
                        .map_err(|_| Error::GitResponse { field: GitField::Branch })?,
                )
            }
            b"detached" => record.detached = true,
            b"prunable" => record.prunable = true,
            b"locked" => {
                record.locked = true;
                if !value.is_empty() {
                    record.reason = Some(
                        String::from_utf8(value.to_vec())
                            .map_err(|_| Error::GitResponse { field: GitField::LockReason })?,
                    );
                }
            }
            _ => {}
        }
    }
    record.path = path.ok_or(Error::GitResponse { field: GitField::WorktreePath })?;
    Ok(record)
}
