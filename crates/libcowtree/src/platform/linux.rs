// Copyright (c) 2026 Windsor Nguyen

//! Linux file cloning owns the exclusively created inode until metadata is ready.

use crate::{
    Error, Operation, Result,
    clone::{cleanup, preserve},
};
use std::{
    fs::{File, Metadata},
    path::Path,
};

pub(crate) fn clone(source: &File, target: &Path, metadata: &Metadata) -> Result<()> {
    let output = File::options()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|error| Error::io(Operation::Open, target, error))?;
    let result = rustix::fs::ioctl_ficlone(&output, source)
        .map_err(|error| Error::io(Operation::Clone, target, error.into()))
        .and_then(|()| {
            preserve(&output, metadata)
                .map_err(|error| Error::io(Operation::Metadata, target, error))
        });
    drop(output);
    result.map_err(|error| cleanup(target, error))
}
