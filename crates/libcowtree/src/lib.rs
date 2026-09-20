// Copyright (c) 2026 Windsor Nguyen

//! Native filesystem operations shared by Cowtree's language interfaces.
//!
//! The filesystem owns block sharing during edits. This library owns clone
//! admission, metadata preservation, and cleanup of destinations it creates.

mod cancellation;
mod clone;
mod creation;
mod entry;
mod error;
mod git;
mod path_json;
mod platform;
mod populate;
mod probe;
mod tree_clone;
mod tree_entry;
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
pub use tree_clone::{clone_tree, populate_tree};
pub use tree_entry::{FileIdentity, TreeEntry, TreeKind};
pub use tree_policy::{CaptureMode, Hardlinks, PathClass, TreePolicy};
pub use tree_scan::scan_tree;
pub use worktree::{AddRequest, Branch, Lock, SourceMode, Worktree};
pub use worktree_error::{WorktreeError, WorktreeResult};
pub use worktree_ops::{list_worktrees, remove_worktree};
