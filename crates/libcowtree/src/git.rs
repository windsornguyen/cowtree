// Copyright (c) 2026 Windsor Nguyen

//! Git remains authoritative for refs, index contents, and worktree registration.
//!
//! Commands receive argument arrays and retain the caller's repository selection.
//! A handle on the common lock serializes native and Python Cowtree clients.

use crate::{FileMode, TrackedFile, Worktree, WorktreeError as Error, WorktreeResult as Result};
use std::{
    ffi::OsStr,
    fs::File,
    path::{Path, PathBuf},
    process::{Command, Output},
};

pub(crate) struct Git {
    pub(crate) root: PathBuf,
}

pub(crate) struct Snapshot {
    pub(crate) commit: String,
    pub(crate) entries: Vec<TrackedFile>,
}

impl Git {
    pub(crate) fn discover(source: Option<&Path>) -> Result<Self> {
        let mut command = Command::new("git");
        if let Some(source) = source {
            command.current_dir(source);
        }
        let output = command
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .map_err(|error| Error::io(source.unwrap_or(Path::new(".")), error))?;
        let root = decode_path(trim_newline(checked(output)?))?;
        let root = root.canonicalize().map_err(|error| Error::io(&root, error))?;
        Ok(Self { root })
    }

    pub(crate) fn command(&self) -> Command {
        let mut command = Command::new("git");
        command.arg("-C").arg(&self.root);
        command
    }

    pub(crate) fn capture(&self, command: &mut Command) -> Result<Vec<u8>> {
        let output = command.output().map_err(|error| Error::io(&self.root, error))?;
        checked(output)
    }

    pub(crate) fn text(&self, command: &mut Command) -> Result<String> {
        let bytes = trim_newline(self.capture(command)?);
        String::from_utf8(bytes).map_err(|_| Error::GitResponse { field: "text" })
    }

    pub(crate) fn head(&self) -> Result<String> {
        self.text(self.command().args(["rev-parse", "HEAD"]))
    }

    pub(crate) fn lock(&self) -> Result<File> {
        let bytes = self.capture(self.command().args([
            "rev-parse",
            "--path-format=absolute",
            "--git-common-dir",
        ]))?;
        let path = decode_path(trim_newline(bytes))?.join("cowtree.lock");
        let file = File::options()
            .read(true)
            .append(true)
            .create(true)
            .open(&path)
            .map_err(|error| Error::io(&path, error))?;
        fs4::FileExt::lock(&file).map_err(|error| Error::io(&path, error))?;
        Ok(file)
    }

    pub(crate) fn status(&self) -> Result<Vec<u8>> {
        let filemode = if cfg!(windows) { "core.filemode=false" } else { "core.filemode=true" };
        self.capture(self.command().args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.ignorestat=false",
            "-c",
            filemode,
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=no",
            "--ignore-submodules=none",
        ]))
    }

    pub(crate) fn snapshot(&self) -> Result<Snapshot> {
        let sparse = self
            .command()
            .args(["config", "--bool", "core.sparseCheckout"])
            .output()
            .map_err(|error| Error::io(&self.root, error))?;
        if !matches!(sparse.status.code(), Some(0 | 1)) {
            checked(sparse)?;
        } else if sparse.stdout == b"true\n" {
            return Err(Error::SparseCheckout);
        }
        let commit = self.head()?;
        let entries = self.entries(OsStr::new(&commit))?;
        let flags = self.capture(self.command().args(["ls-files", "-v", "-z"]))?;
        if flags.split(|byte| *byte == 0).any(|record| {
            record.first().is_some_and(|flag| flag.is_ascii_lowercase() || *flag == b'S')
        }) {
            return Err(Error::DirtySource);
        }
        if !self.status()?.is_empty() {
            return Err(Error::DirtySource);
        }
        Ok(Snapshot { commit, entries })
    }

    pub(crate) fn resolve(&self, revision: &OsStr) -> Result<String> {
        let mut selected = revision.to_os_string();
        selected.push("^{commit}");
        self.text(self.command().args(["rev-parse", "--verify", "--end-of-options"]).arg(selected))
    }

    pub(crate) fn entries(&self, revision: &OsStr) -> Result<Vec<TrackedFile>> {
        let data = self.capture(self.command().args(["ls-tree", "-r", "-z"]).arg(revision))?;
        data.split(|byte| *byte == 0).filter(|record| !record.is_empty()).map(parse_entry).collect()
    }

    pub(crate) fn worktrees(&self) -> Result<Vec<Worktree>> {
        let data = self.capture(self.command().args(["worktree", "list", "--porcelain", "-z"]))?;
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
            return Err(Error::GitResponse { field: "worktree terminator" });
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

fn checked(output: Output) -> Result<Vec<u8>> {
    if output.status.success() {
        return Ok(output.stdout);
    }
    Err(Error::Git {
        status: output.status,
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn trim_newline(mut bytes: Vec<u8>) -> Vec<u8> {
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    bytes
}

pub(crate) fn decode_path(bytes: Vec<u8>) -> Result<PathBuf> {
    #[cfg(unix)]
    {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};
        Ok(OsString::from_vec(bytes).into())
    }
    #[cfg(windows)]
    {
        String::from_utf8(bytes)
            .map(PathBuf::from)
            .map_err(|_| Error::GitResponse { field: "path encoding" })
    }
}

fn parse_entry(record: &[u8]) -> Result<TrackedFile> {
    let tab = record
        .iter()
        .position(|byte| *byte == b'\t')
        .ok_or(Error::GitResponse { field: "tree entry" })?;
    let path = decode_path(record[tab + 1..].to_vec())?;
    let mode = record[..tab]
        .split(|byte| *byte == b' ')
        .next()
        .ok_or(Error::GitResponse { field: "tree mode" })?;
    let mode = match mode {
        b"100644" => FileMode::Regular,
        b"100755" => FileMode::Executable,
        b"120000" => FileMode::Symlink,
        b"160000" => return Err(Error::Submodule { path }),
        _ => return Err(Error::UnsupportedMode { path }),
    };
    Ok(TrackedFile::new(path, mode)?)
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
        let key = pair.next().ok_or(Error::GitResponse { field: "worktree key" })?;
        let value = pair.next().unwrap_or_default();
        match key {
            b"worktree" => path = Some(decode_path(value.to_vec())?),
            b"HEAD" => {
                record.head = Some(
                    String::from_utf8(value.to_vec())
                        .map_err(|_| Error::GitResponse { field: "HEAD" })?,
                )
            }
            b"branch" => {
                record.branch = Some(
                    String::from_utf8(value.to_vec())
                        .map_err(|_| Error::GitResponse { field: "branch" })?,
                )
            }
            b"detached" => record.detached = true,
            b"prunable" => record.prunable = true,
            b"locked" => {
                record.locked = true;
                if !value.is_empty() {
                    record.reason = Some(
                        String::from_utf8(value.to_vec())
                            .map_err(|_| Error::GitResponse { field: "lock reason" })?,
                    );
                }
            }
            _ => {}
        }
    }
    record.path = path.ok_or(Error::GitResponse { field: "worktree path" })?;
    Ok(record)
}
