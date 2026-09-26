// Copyright (c) 2026 Windsor Nguyen

//! Establish clone support by comparing a private pair before and after writes.

use crate::{
    Error, ProbeInvariant, RequestIssue, WorktreeError as WorkflowError, WorktreeResult, clone_file,
};
use serde::Serialize;
use std::{
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub submodules: crate::SubmodulePolicy,
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
    let path = path.canonicalize().map_err(|error| match error.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => {
            WorkflowError::InvalidRequest { reason: RequestIssue::ProbeDirectory }
        }
        _ => WorkflowError::io(path, error),
    })?;
    let metadata = fs::metadata(&path).map_err(|error| WorkflowError::io(&path, error))?;
    if !metadata.is_dir() {
        return Err(WorkflowError::InvalidRequest { reason: RequestIssue::ProbeDirectory });
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
            submodules: crate::SubmodulePolicy::default(),
            path,
            filesystem: backend.0,
            clone_tool: Some(backend.1),
            reason: None,
        }),
        Err(WorkflowError::Native(Error::UnsupportedPlatform)) => Ok(DoctorReport {
            submodules: crate::SubmodulePolicy::default(),
            path,
            filesystem: "unsupported",
            clone_tool: None,
            reason: Some("platform lacks a native clone implementation".into()),
        }),
        Err(WorkflowError::Native(Error::Io { source, .. }))
            if crate::error::clone_unavailable(&source) =>
        {
            Ok(DoctorReport {
                submodules: crate::SubmodulePolicy::default(),
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
        return Err(WorkflowError::ProbeFailed { invariant: ProbeInvariant::ClonedContents });
    }
    fs::write(&target, b"target edit").map_err(|error| WorkflowError::io(&target, error))?;
    if fs::read(&source).map_err(|error| WorkflowError::io(&source, error))? != b"cowtree probe\n" {
        return Err(WorkflowError::ProbeFailed { invariant: ProbeInvariant::SourceIsolation });
    }
    fs::write(&source, b"source edit").map_err(|error| WorkflowError::io(&source, error))?;
    if fs::read(&target).map_err(|error| WorkflowError::io(&target, error))? != b"target edit" {
        return Err(WorkflowError::ProbeFailed { invariant: ProbeInvariant::TargetIsolation });
    }
    Ok(())
}
