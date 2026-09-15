// Copyright (c) 2026 Windsor Nguyen

//! Local fenced publication with SQLite metadata and durable immutable objects.
//!
//! See the crate README for the authority, retention, and failure contracts.

mod database;
mod durability;
mod error;
mod fault;
mod leases;
mod maintenance;
pub mod objects;
mod publication;
mod types;

pub use database::Store;
pub use error::{Error, LimitKind, Result};
pub use types::{
    Candidate, Entry, EntryKind, Grant, LeafId, LeafView, Limits, Maintenance, Proposal,
    ProposalInput, ProposedChange, Receipt, RequestId, ResourcePath, Snapshot, Token, Version,
};
