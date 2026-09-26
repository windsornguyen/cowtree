// Copyright (c) 2026 Windsor Nguyen

//! Durable JSON records publish complete bytes before their authoritative names.

use std::{
    fs::{self, File},
    io::Write,
    path::Path,
};

use serde::{Serialize, de::DeserializeOwned};

use crate::{Error, Result};

pub(crate) fn read<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = fs::read(path).map_err(|error| Error::io(path, error))?;
    serde_json::from_slice(&bytes).map_err(|error| Error::record(path, error))
}

pub(crate) fn write<T: Serialize>(path: &Path, record: &T) -> Result<()> {
    let parent =
        path.parent().ok_or_else(|| Error::io(path, std::io::ErrorKind::InvalidInput.into()))?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".pending-")
        .tempfile_in(parent)
        .map_err(|error| Error::io(parent, error))?;
    serde_json::to_writer(&mut temporary, record).map_err(|error| Error::record(path, error))?;
    temporary.flush().map_err(|error| Error::io(path, error))?;
    sync_file(temporary.as_file()).map_err(|error| Error::io(path, error))?;
    temporary.persist(path).map_err(|error| Error::io(path, error.error))?;
    sync_directory(parent)
}

pub(crate) fn sync_file(file: &File) -> std::io::Result<()> {
    file.sync_all()?;
    #[cfg(target_os = "macos")]
    rustix::fs::fcntl_fullfsync(file)?;
    Ok(())
}

pub(crate) fn sync_directory(path: &Path) -> Result<()> {
    let file = File::open(path).map_err(|error| Error::io(path, error))?;
    file.sync_all().map_err(|error| Error::io(path, error))
}

pub(crate) fn remove_directory(path: &Path) -> Result<()> {
    fs::remove_dir_all(path).map_err(|error| Error::io(path, error))?;
    if let Some(parent) = path.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}
