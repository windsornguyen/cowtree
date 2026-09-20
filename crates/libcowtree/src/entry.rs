// Copyright (c) 2026 Windsor Nguyen

//! Validated Git-relative paths keep clone requests outside repository control files.

use crate::{Error, Result};
use std::path::{Component, Path, PathBuf};

/// Supported Git entry kinds distinguish executable files and symbolic links.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileMode {
    Regular,
    Executable,
    Symlink,
}

/// A tracked pathname cannot be absolute, traverse parents, or name control state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackedFile {
    path: PathBuf,
    mode: FileMode,
}

impl TrackedFile {
    pub fn new(path: PathBuf, mode: FileMode) -> Result<Self> {
        if path.as_os_str().is_empty() || path.as_os_str().as_encoded_bytes().contains(&0) {
            return Err(Error::InvalidTrackedPath { path });
        }
        for component in path.components() {
            let Component::Normal(name) = component else {
                return Err(Error::InvalidTrackedPath { path });
            };
            if name.eq_ignore_ascii_case(".git") || name.eq_ignore_ascii_case(".cowtree") {
                return Err(Error::InvalidTrackedPath { path });
            }
        }
        Ok(Self { path, mode })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn mode(&self) -> FileMode {
        self.mode
    }
}

#[cfg(test)]
mod tests {
    use super::{FileMode, TrackedFile};

    #[test]
    fn tracked_paths_cannot_escape_or_replace_control_state() {
        for path in [
            "",
            ".",
            "..",
            "../escape",
            "/absolute",
            ".git/config",
            "a/.GIT/config",
            ".cowtree/state",
        ] {
            assert!(TrackedFile::new(path.into(), FileMode::Regular).is_err(), "{path}");
        }
    }
}
