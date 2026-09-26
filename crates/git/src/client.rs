// Copyright (c) 2026 Windsor Nguyen

//! Select Git context and retain transaction locks.

use crate::{Error, Result};
use std::{
    fs::File,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};

#[derive(Clone)]
pub struct Client {
    /// Selected checkout or common Git directory.
    pub root: PathBuf,
    /// Descriptors retained by mutating child processes.
    pub locks: Vec<Arc<File>>,
}

impl Client {
    #[must_use]
    pub fn at(root: &Path) -> Self {
        Self { root: root.into(), locks: Vec::new() }
    }

    pub fn discover(source: Option<&Path>) -> Result<Self> {
        let selected = Self::at(source.unwrap_or(Path::new(".")));
        let bytes = selected.capture(selected.command()?.args(["rev-parse", "--show-toplevel"]))?;
        let root = decode_path(trim_newline(bytes))?;
        let root = root.canonicalize().map_err(|error| Error::io(&root, error))?;
        Ok(Self::at(&root))
    }

    #[must_use]
    pub fn select(&self, root: &Path) -> Self {
        Self { root: root.into(), locks: self.locks.clone() }
    }

    pub fn command(&self) -> Result<Command> {
        let mut command = Command::new("git");
        command.arg("-C").arg(dunce::simplified(&self.root));
        #[cfg(unix)]
        {
            use command_fds::CommandFdExt;
            use std::os::fd::AsFd;
            if self.locks.is_empty() {
                return Ok(command);
            }
            let descriptors = self
                .locks
                .iter()
                .map(|file| file.as_fd().try_clone_to_owned())
                .collect::<std::io::Result<Vec<_>>>()
                .map_err(|error| Error::io(&self.root, error))?;
            command.preserved_fds(descriptors);
        }
        Ok(command)
    }

    pub fn directory(&self) -> Result<PathBuf> {
        let bytes = self.capture(self.command()?.args([
            "rev-parse",
            "--path-format=absolute",
            "--git-common-dir",
        ]))?;
        decode_path(trim_newline(bytes))
    }

    pub fn lock(&self) -> Result<Arc<File>> {
        acquire(&self.directory()?)
    }

    pub fn locked_at(&self, directory: &Path) -> Result<Self> {
        let mut selected = self.clone();
        selected.locks.push(acquire(directory)?);
        Ok(selected)
    }

    pub fn head(&self) -> Result<String> {
        self.text(self.command()?.args(["rev-parse", "HEAD"]))
    }

    pub fn status(&self) -> Result<Vec<u8>> {
        let filemode = if cfg!(windows) { "core.filemode=false" } else { "core.filemode=true" };
        self.capture(self.command()?.args([
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
}

fn acquire(directory: &Path) -> Result<Arc<File>> {
    let path = directory.join("cowtree.lock");
    let file = File::options()
        .read(true)
        .append(true)
        .create(true)
        .open(&path)
        .map_err(|error| Error::io(&path, error))?;
    fs4::FileExt::lock(&file).map_err(|error| Error::io(&path, error))?;
    Ok(Arc::new(file))
}

pub(crate) fn trim_newline(mut bytes: Vec<u8>) -> Vec<u8> {
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    bytes
}

pub fn decode_path(bytes: Vec<u8>) -> Result<PathBuf> {
    #[cfg(unix)]
    {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};
        Ok(OsString::from_vec(bytes).into())
    }
    #[cfg(windows)]
    {
        String::from_utf8(bytes).map(PathBuf::from).map_err(|source| Error::Encoding { source })
    }
}
