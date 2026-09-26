// Copyright (c) 2026 Windsor Nguyen

//! Managed filesystem transactions over native clones, Git, and SQLite authority.
//!
//! Journals own filesystem recovery. The metadata crate owns publication fencing.
//! Git remains authoritative for refs, indexes, and linked worktree registration.

#![cfg(unix)]

mod batches;
mod captures;
mod checks;
mod collection;
mod durability;
mod error;
mod fault;
mod git;
mod identity;
mod images;
mod imports;
mod installation;
mod leaves;
mod lifecycle;
mod manifest;
mod nodes;
mod paths;
mod policy;
mod projection;
mod publications;
mod records;
mod recovery;
mod resolution;
mod seals;
mod session;
mod submodules;
mod types;
mod views;
mod workspace;

pub use error::{Error, Result};
pub use identity::{CommitId, NodeId};
pub use types::{Config, Hardlinks, Leaf, Node, Pending, Pin, Policy, SubmodulePolicy, Validation};

pub use workspace::{CreateRequest, Workspace};

pub use checks::CheckRequest;

pub use lifecycle::DropPolicy;

pub use resolution::Choice;

pub use collection::Collection;

pub use seals::Retention;
