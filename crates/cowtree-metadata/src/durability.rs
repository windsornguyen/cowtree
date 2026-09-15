// Copyright (c) 2026 Windsor Nguyen

//! Filesystem flush boundaries for immutable object publication.
//!
//! Files are synced before their final names are linked; the containing directory is
//! synced after publication or removal. macOS additionally requests `F_FULLFSYNC`
//! through rustix's safe wrapper. Success proves completion of these OS requests,
//! not behavior of faulty hardware or an independently tested power-cut guarantee.

use std::fs::File;
use std::io;
use std::path::Path;

/// Flush file contents and metadata through the platform's persistence interface.
pub fn sync_file(file: &File) -> io::Result<()> {
    file.sync_all()?;
    #[cfg(target_os = "macos")]
    rustix::fs::fcntl_fullfsync(file)?;
    Ok(())
}

/// Flush directory entry changes after a name is created, published, or removed.
pub fn sync_directory(path: &Path) -> io::Result<()> {
    sync_file(&File::open(path)?)
}
