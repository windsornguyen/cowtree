// Copyright (c) 2026 Windsor Nguyen

//! Filesystem flush boundaries for immutable object publication.
//!
//! Files are synced before their final names are linked; the containing directory is
//! synced after publication or removal. macOS requests `F_FULLFSYNC` once through
//! rustix's safe wrapper; other platforms use std. Success covers OS request completion.
//! Hardware fault tolerance and recovery after power cuts need separate qualification.

use std::fs::File;
use std::io;
use std::path::Path;

/// Flush file contents and metadata through the platform's persistence interface.
pub fn sync_file(file: &File) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    let result = rustix::fs::fcntl_fullfsync(file).map_err(Into::into);
    #[cfg(not(target_os = "macos"))]
    let result = file.sync_all();
    result
}

/// Flush directory entry changes after a name is created, published, or removed.
pub fn sync_directory(path: &Path) -> io::Result<()> {
    sync_file(&File::open(path)?)
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn unsupported_full_flush_returns_an_error() -> io::Result<()> {
        let file = File::open("/dev/null")?;
        assert!(sync_file(&file).is_err());
        Ok(())
    }
}
