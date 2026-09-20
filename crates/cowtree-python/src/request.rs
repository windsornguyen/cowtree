// Copyright (c) 2026 Windsor Nguyen

//! Convert the public Python request into an explicit native branch and source policy.

use cowtree::{AddRequest, Branch, Lock, RequestIssue, SourceMode, WorktreeError};
use pyo3::FromPyObject;
use std::path::PathBuf;

#[derive(FromPyObject)]
pub(crate) struct Request {
    path: PathBuf,
    source: Option<PathBuf>,
    branch: Option<String>,
    existing_branch: Option<String>,
    commitish: String,
    detach: bool,
    lock: bool,
    reason: Option<String>,
    source_mode: String,
}

impl Request {
    pub(crate) fn native(self) -> cowtree::WorktreeResult<AddRequest> {
        let mut request = AddRequest::new(self.path);
        request.source = self.source;
        request.revision = self.commitish.into();
        request.branch = match (self.branch, self.existing_branch, self.detach) {
            (Some(_), Some(_), _) | (Some(_), None, true) | (None, Some(_), true) => {
                return Err(WorktreeError::InvalidRequest { reason: RequestIssue::BranchConflict });
            }
            (Some(branch), None, false) => Branch::New(branch.into()),
            (None, Some(branch), false) => Branch::Existing(branch.into()),
            (None, None, _) => Branch::Detached,
        };
        request.source_mode = match self.source_mode.as_str() {
            "checkout" => SourceMode::Checkout,
            "commit" => SourceMode::Committed,
            _ => return Err(WorktreeError::InvalidRequest { reason: RequestIssue::SourceMode }),
        };
        request.lock = match (self.lock, self.reason) {
            (false, None) => Lock::Release,
            (true, reason) => Lock::Retain { reason },
            (false, Some(_)) => {
                return Err(WorktreeError::InvalidRequest {
                    reason: RequestIssue::ReasonWithoutLock,
                });
            }
        };
        Ok(request)
    }
}
