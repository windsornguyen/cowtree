// Copyright (c) 2026 Windsor Nguyen

//! Establish clone support by comparing a private pair before and after writes.

use crate::{Error, WorktreeError as WorkflowError, WorktreeResult, clone_file};
use serde::Serialize;
use std::{
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    #[serde(serialize_with = "crate::path_json::path")]
    pub path: PathBuf,
    pub filesystem: &'static str,
    pub clone_tool: Option<&'static str>,
    pub reason: Option<String>,
}

impl DoctorReport {
    #[must_use]
    pub fn supported(&self) -> bool {
        self.clone_tool.is_some()
    }
}

pub fn inspect_path(path: &Path) -> WorktreeResult<DoctorReport> {
    let path = path.canonicalize().map_err(|_| WorkflowError::InvalidRequest {
        reason: "doctor requires an existing directory",
    })?;
    if !path.is_dir() {
        return Err(WorkflowError::InvalidRequest { reason: "doctor requires a directory" });
    }
    let directory = tempfile::Builder::new()
        .prefix(".cowtree-probe-")
        .tempdir_in(&path)
        .map_err(|error| WorkflowError::io(&path, error))?;
    let result = probe(directory.path());
    let temporary = directory.path().to_path_buf();
    directory.close().map_err(|error| WorkflowError::io(&temporary, error))?;
    let backend = if cfg!(target_os = "macos") {
        ("clonefile", "clonefile(2)")
    } else if cfg!(target_os = "linux") {
        ("reflink", "FICLONE")
    } else {
        ("block_clone", "FSCTL_DUPLICATE_EXTENTS_TO_FILE")
    };
    match result {
        Ok(()) => Ok(DoctorReport {
            path,
            filesystem: backend.0,
            clone_tool: Some(backend.1),
            reason: None,
        }),
        Err(WorkflowError::Native(Error::UnsupportedPlatform)) => Ok(DoctorReport {
            path,
            filesystem: "unsupported",
            clone_tool: None,
            reason: Some("platform lacks a native clone implementation".into()),
        }),
        Err(WorkflowError::Native(Error::Io { source, .. }))
            if matches!(
                source.kind(),
                io::ErrorKind::Unsupported | io::ErrorKind::CrossesDevices
            ) =>
        {
            Ok(DoctorReport {
                path,
                filesystem: backend.0,
                clone_tool: None,
                reason: Some(source.to_string()),
            })
        }
        Err(error) => Err(error),
    }
}

fn probe(directory: &Path) -> WorktreeResult<()> {
    let source = directory.join("source");
    let target = directory.join("target");
    fs::write(&source, b"cowtree probe\n").map_err(|error| WorkflowError::io(&source, error))?;
    clone_file(&source, &target)?;
    let bytes = fs::read(&target).map_err(|error| WorkflowError::io(&target, error))?;
    if bytes != b"cowtree probe\n"
        || same_file::is_same_file(&source, &target)
            .map_err(|error| WorkflowError::io(&target, error))?
    {
        return Err(WorkflowError::InvalidRequest {
            reason: "native clone failed content or identity verification",
        });
    }
    fs::write(&target, b"target edit").map_err(|error| WorkflowError::io(&target, error))?;
    if fs::read(&source).map_err(|error| WorkflowError::io(&source, error))? != b"cowtree probe\n" {
        return Err(WorkflowError::InvalidRequest { reason: "native clone modified its source" });
    }
    fs::write(&source, b"source edit").map_err(|error| WorkflowError::io(&source, error))?;
    if fs::read(&target).map_err(|error| WorkflowError::io(&target, error))? != b"target edit" {
        return Err(WorkflowError::InvalidRequest { reason: "source write modified its clone" });
    }
    Ok(())
}
