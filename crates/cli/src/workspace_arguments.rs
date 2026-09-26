// Copyright (c) 2026 Windsor Nguyen

//! Parse managed operations into explicit Rust domain requests.

use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};
use cowtree_metadata::LeafId;
use cowtree_workspace::NodeId;

#[derive(Args)]
pub(crate) struct Options {
    /// Managed store whose transitions this command owns.
    #[arg(long)]
    pub root: PathBuf,
    /// One complete managed operation.
    #[command(subcommand)]
    pub action: Action,
}

#[derive(Subcommand)]
pub(crate) enum Action {
    Version,
    Init {
        /// Checkout supplying source and selected caches.
        #[arg(long)]
        source: PathBuf,
        /// Cache prefixes to preserve in snapshots.
        #[arg(long)]
        derived: Vec<String>,
        /// Prefixes excluded from snapshots.
        #[arg(long)]
        ephemeral: Vec<String>,
        /// Whether cache aliases receive independent clones.
        #[arg(long, value_enum, default_value = "reject")]
        derived_hardlinks: Hardlinks,
        /// Explicit admission policy for dependencies.
        #[arg(long, value_enum, default_value = "reject")]
        submodules: Submodules,
    },
    ImportStatus,
    List,
    Fork {
        /// Absent sibling worktree destination.
        path: PathBuf,
        /// Retained checkpoint, or the published warm tip when omitted.
        #[arg(long, value_parser = node)]
        node: Option<NodeId>,
    },
    Acquire(PathSelection),
    Sync(Identity),
    Discard(PathSelection),
    Seal(Identity),
    Retain(Checkpoint),
    Release(Checkpoint),
    Capture(Identity),
    Prepare(Identity),
    Check {
        /// Leaf owning the candidate.
        #[arg(value_parser = leaf)]
        identity: LeafId,
        /// Maximum wall-clock time in seconds.
        #[arg(long, default_value_t = 300)]
        timeout: u64,
        /// Check argv after --.
        #[arg(last = true, required = true)]
        command: Vec<String>,
    },
    Commit(Identity),
    Result {
        /// Leaf owning the request.
        #[arg(value_parser = leaf)]
        identity: LeafId,
        /// Exact capture sequence being queried.
        sequence: u64,
    },
    Abort(Identity),
    Drop {
        /// Leaf to retire.
        #[arg(value_parser = leaf)]
        identity: LeafId,
        /// Explicit consent to discard private source edits.
        #[arg(long)]
        force: bool,
    },
    Collect,
    Recover,
    Log,
    Resolve {
        /// Leaf containing stale private edits.
        #[arg(value_parser = leaf)]
        identity: LeafId,
        /// JSON map selecting local or published for each path.
        #[arg(long)]
        choices: PathBuf,
    },
    PrepareBatch {
        /// Distinct captured leaves, published as one union.
        #[arg(required = true, value_parser = leaf)]
        identities: Vec<LeafId>,
    },
    CheckBatch {
        /// Exact candidate record from prepare-batch.
        #[arg(long)]
        candidate: PathBuf,
        /// Maximum wall-clock time in seconds.
        #[arg(long, default_value_t = 300)]
        timeout: u64,
        /// Check argv after --.
        #[arg(last = true, required = true)]
        command: Vec<String>,
    },
    CommitBatch {
        /// Exact checked candidate record.
        #[arg(long)]
        candidate: PathBuf,
    },
}

#[derive(Args)]
pub(crate) struct Identity {
    /// Monotone authority leaf identity.
    #[arg(value_parser = leaf)]
    pub identity: LeafId,
}

#[derive(Args)]
pub(crate) struct PathSelection {
    /// Leaf whose origins are selected.
    #[arg(value_parser = leaf)]
    pub identity: LeafId,
    /// Explicit relative source paths.
    #[arg(required = true)]
    pub paths: Vec<String>,
}

#[derive(Args)]
pub(crate) struct Checkpoint {
    /// Retained checkpoint identity.
    #[arg(value_parser = node)]
    pub node: NodeId,
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum Hardlinks {
    Reject,
    Clone,
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum Submodules {
    Reject,
    MaterializePinned,
}

fn leaf(value: &str) -> Result<LeafId, String> {
    let value = value.parse::<u64>().map_err(|error| error.to_string())?;
    LeafId::new(value).map_err(|error| error.to_string())
}

fn node(value: &str) -> Result<NodeId, String> {
    NodeId::parse(value).map_err(|error| error.to_string())
}
