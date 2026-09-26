// Copyright (c) 2026 Windsor Nguyen

//! Preserve command failures and their original causes.

use std::{
    io,
    path::{Path, PathBuf},
    process::ExitStatus,
    string::FromUtf8Error,
};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Git I/O at {path}: {source}")]
    Io {
        /// Selected resource or command directory.
        path: PathBuf,
        /// Original operating-system failure.
        #[source]
        source: io::Error,
    },
    #[error("Git failed ({status}): {stderr}")]
    Command {
        /// Exit code or terminating signal.
        status: ExitStatus,
        /// Git's diagnostic output.
        stderr: String,
    },
    #[error("Git returned non-UTF-8 text: {source}")]
    Encoding {
        /// Original invalid output bytes.
        #[source]
        source: FromUtf8Error,
    },
}

impl Error {
    pub(crate) fn io(path: &Path, source: io::Error) -> Self {
        Self::Io { path: path.into(), source }
    }
}
