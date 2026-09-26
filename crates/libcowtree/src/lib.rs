// Copyright (c) 2026 Windsor Nguyen

//! Standalone worktrees and native filesystem operations.
//!
//! The filesystem owns block sharing during edits. This library owns clone
//! admission, metadata preservation, and cleanup of destinations it creates.

mod cancellation;
mod clone;
mod creation;
mod entry;
mod error;
mod git;
mod git_tree;
mod path_json;
mod platform;
mod populate;
mod probe;
mod submodule_objects;
mod submodule_record;
mod submodules;
mod tree_clone;
mod tree_entry;
#[cfg(unix)]
mod tree_links;
mod tree_policy;
mod tree_scan;
mod worktree;
mod worktree_error;
mod worktree_ops;

pub use cancellation::Cancellation;
pub use clone::clone_file;
pub use creation::{PreparedAdd, add_worktree};
pub use entry::{FileMode, TrackedFile};
pub use error::{Error, Operation, Result};
pub use populate::populate_tracked;
pub use probe::{DoctorReport, inspect_path};
pub use submodule_record::submodule_policy;
pub use tree_clone::{clone_tree, populate_tree};
pub use tree_entry::{FileIdentity, TreeEntry, TreeKind};
pub use tree_policy::{CaptureMode, Hardlinks, PathClass, TreePolicy};
pub use tree_scan::scan_tree;
pub use worktree::{AddRequest, Branch, Lock, RequestIssue, SourceMode, SubmodulePolicy, Worktree};
pub use worktree_error::{GitField, ProbeInvariant, WorktreeError, WorktreeResult};
pub use worktree_ops::{list_worktrees, remove_worktree};
