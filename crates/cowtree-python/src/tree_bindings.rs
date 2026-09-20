// Copyright (c) 2026 Windsor Nguyen

//! Convert tree requests once and keep traversal and hashing inside libcowtree.

use pyo3::prelude::*;
use std::{ffi::OsString, path::PathBuf};

#[derive(FromPyObject)]
pub(crate) struct Policy {
    derived: Vec<OsString>,
    ephemeral: Vec<OsString>,
    ignored: Vec<OsString>,
    derived_hardlinks: String,
}

impl Policy {
    fn native(self) -> PyResult<cowtree::TreePolicy> {
        let hardlinks = match self.derived_hardlinks.as_str() {
            "reject" => cowtree::Hardlinks::Reject,
            "clone" => cowtree::Hardlinks::Clone,
            _ => {
                return Err(crate::bindings::EngineError::new_err((
                    "invalid_arguments",
                    "invalid hardlink policy",
                )));
            }
        };
        cowtree::TreePolicy::new(&self.derived, &self.ephemeral, &self.ignored, hardlinks)
            .map_err(filesystem_error)
    }
}

fn filesystem_error(error: cowtree::Error) -> PyErr {
    crate::bindings::EngineError::new_err((error.code(), error.to_string()))
}

fn capture(value: &str) -> PyResult<cowtree::CaptureMode> {
    match value {
        "content" => Ok(cowtree::CaptureMode::Content),
        "metadata" => Ok(cowtree::CaptureMode::Metadata),
        _ => Err(crate::bindings::EngineError::new_err((
            "invalid_arguments",
            "invalid capture mode",
        ))),
    }
}

#[pyfunction]
pub(crate) fn validate_policy(policy: Policy) -> PyResult<()> {
    policy.native().map(|_| ())
}

#[pyfunction]
pub(crate) fn classify_path(path: OsString, policy: Policy) -> PyResult<String> {
    let policy = policy.native()?;
    crate::bindings::json(policy.classify(&path))
}

#[pyfunction]
pub(crate) fn scan_tree(
    py: Python<'_>,
    root: PathBuf,
    policy: Policy,
    mode: &str,
) -> PyResult<String> {
    let policy = policy.native()?;
    let mode = capture(mode)?;
    let entries =
        py.detach(move || cowtree::scan_tree(&root, &policy, mode)).map_err(filesystem_error)?;
    crate::bindings::json(entries)
}

#[pyfunction]
pub(crate) fn clone_tree(
    py: Python<'_>,
    source: PathBuf,
    target: PathBuf,
    policy: Policy,
) -> PyResult<String> {
    let policy = policy.native()?;
    let entries = py
        .detach(move || cowtree::clone_tree(&source, &target, &policy))
        .map_err(filesystem_error)?;
    crate::bindings::json(entries)
}

#[pyfunction]
pub(crate) fn populate_tree(
    py: Python<'_>,
    source: PathBuf,
    target: PathBuf,
    policy: Policy,
    mode: &str,
) -> PyResult<String> {
    let policy = policy.native()?;
    let mode = capture(mode)?;
    let entries = py
        .detach(move || cowtree::populate_tree(&source, &target, &policy, mode))
        .map_err(filesystem_error)?;
    crate::bindings::json(entries)
}
