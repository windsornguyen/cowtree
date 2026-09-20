// Copyright (c) 2026 Windsor Nguyen

//! Release Python's interpreter lock while the native engine owns the operation.

use crate::request::Request;
use pyo3::{create_exception, exceptions::PyRuntimeError, prelude::*};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

create_exception!(_libcowtree, EngineError, PyRuntimeError);

fn engine_error(error: cowtree::WorktreeError) -> PyErr {
    EngineError::new_err((error.code().to_owned(), error.to_string()))
}

pub(crate) fn json<T: Serialize>(value: T) -> PyResult<String> {
    serde_json::to_string(&value)
        .map_err(|error| EngineError::new_err(("command_failed", error.to_string())))
}

#[pyfunction]
fn add_worktree(py: Python<'_>, request: Request) -> PyResult<String> {
    let mut request = request.native().map_err(engine_error)?;
    let interrupted = Arc::new(OnceLock::new());
    let pending = Arc::clone(&interrupted);
    request.cancellation =
        cowtree::Cancellation::with_check(move || match Python::attach(|py| py.check_signals()) {
            Ok(()) => false,
            Err(error) => {
                pending.get_or_init(|| error);
                true
            }
        });
    let result = py.detach(move || cowtree::add_worktree(&request));
    match result {
        Ok(tree) => json(tree),
        Err(error) => {
            let signal = interrupted.get().map(|error| error.clone_ref(py));
            if matches!(error, cowtree::WorktreeError::Native(cowtree::Error::Cancelled)) {
                if let Some(signal) = signal {
                    return Err(signal);
                }
            }
            let failure = engine_error(error);
            failure.set_cause(py, signal);
            Err(failure)
        }
    }
}

#[pyfunction]
#[pyo3(signature = (source=None))]
fn list_worktrees(py: Python<'_>, source: Option<PathBuf>) -> PyResult<String> {
    let trees =
        py.detach(move || cowtree::list_worktrees(source.as_deref())).map_err(engine_error)?;
    json(trees)
}

#[pyfunction]
#[pyo3(signature = (path, source=None, force=false))]
fn remove_worktree(
    py: Python<'_>,
    path: PathBuf,
    source: Option<PathBuf>,
    force: bool,
) -> PyResult<()> {
    py.detach(move || cowtree::remove_worktree(&path, source.as_deref(), force))
        .map_err(engine_error)
}

#[pyfunction]
fn inspect_path(py: Python<'_>, path: PathBuf) -> PyResult<String> {
    let report = py.detach(move || cowtree::inspect_path(&path)).map_err(engine_error)?;
    json(report)
}

#[pyfunction]
fn clone_file(py: Python<'_>, source: PathBuf, target: PathBuf) -> PyResult<()> {
    py.detach(move || cowtree::clone_file(&source, &target))
        .map_err(|error| engine_error(cowtree::WorktreeError::Native(error)))
}

#[pymodule]
fn _libcowtree(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add("EngineError", module.py().get_type::<EngineError>())?;
    module.add_function(wrap_pyfunction!(add_worktree, module)?)?;
    module.add_function(wrap_pyfunction!(list_worktrees, module)?)?;
    module.add_function(wrap_pyfunction!(remove_worktree, module)?)?;
    module.add_function(wrap_pyfunction!(inspect_path, module)?)?;
    module.add_function(wrap_pyfunction!(clone_file, module)?)?;
    module.add_function(wrap_pyfunction!(crate::tree_bindings::validate_policy, module)?)?;
    module.add_function(wrap_pyfunction!(crate::tree_bindings::classify_path, module)?)?;
    module.add_function(wrap_pyfunction!(crate::tree_bindings::scan_tree, module)?)?;
    module.add_function(wrap_pyfunction!(crate::tree_bindings::clone_tree, module)?)?;
    module.add_function(wrap_pyfunction!(crate::tree_bindings::populate_tree, module)?)?;
    Ok(())
}
