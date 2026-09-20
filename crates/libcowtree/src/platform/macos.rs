// Copyright (c) 2026 Windsor Nguyen

//! macOS clones directly from an open source descriptor into an absent path.

use crate::{Error, Operation, Result, clone::cleanup};
use std::{
    fs::{self, File, Metadata},
    os::unix::fs::PermissionsExt,
    path::Path,
};

pub(crate) fn clone(source: &File, target: &Path, metadata: &Metadata) -> Result<()> {
    rustix::fs::fclonefileat(source, rustix::fs::CWD, target, rustix::fs::CloneFlags::empty())
        .map_err(|error| Error::io(Operation::Clone, target, error.into()))?;
    // clonefile preserves times and ordinary mode bits, but clears setuid and setgid.
    if metadata.permissions().mode() & 0o6000 != 0 {
        fs::set_permissions(target, metadata.permissions())
            .map_err(|error| cleanup(target, Error::io(Operation::Metadata, target, error)))?;
    }
    Ok(())
}
