// Copyright (c) 2026 Windsor Nguyen

//! Decode only the reviewed TLC export schema before creating any runtime state.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, path::Path};

pub type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Fork,
    Acquire,
    Write,
    Propose,
    Commit,
    Sync,
    Expire,
    Discard,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    /// Reviewed mapping from a model transition to the public runtime API.
    pub action: Action,
    /// Symbolic leaf name used in the checker's state.
    pub leaf: String,
    /// Source key selected by this transition.
    #[serde(default = "default_path")]
    pub path: String,
    /// Symbolic file bytes selected by a write.
    #[serde(default = "default_value")]
    pub value: String,
}
fn default_path() -> String {
    "p1".into()
}
fn default_value() -> String {
    "v1".into()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Version of the reviewed action mapping.
    schema_version: u8,
    /// Exact vendored checker release.
    checker_version: String,
    /// Digest binding the checker executable.
    checker_sha256: String,
    /// Digests binding every model and configuration input.
    inputs: BTreeMap<String, String>,
    /// Digest of the unmodified TLC JSON export.
    export_sha256: String,
    /// Actions whose postconditions are read from the export.
    pub actions: Vec<Step>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    /// Symbolic request owner.
    pub leaf: String,
    /// Frozen requested values, including unselected None sentinels.
    pub delta: BTreeMap<String, String>,
    /// Captured model tokens, validated as numbers but not equated to global runtime counters.
    #[serde(rename = "tokens")]
    _tokens: BTreeMap<String, u64>,
    /// Captured published origins.
    pub origin: BTreeMap<String, String>,
    /// Model epoch from which the request was captured.
    #[serde(rename = "base")]
    _base: u64,
    /// Model's stale-path annotation.
    #[serde(rename = "stale")]
    _stale: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct State {
    /// Installed comparison origins for every symbolic leaf.
    pub origin: BTreeMap<String, BTreeMap<String, String>>,
    /// Active modeled leaves.
    pub active: BTreeMap<String, bool>,
    /// Private edits relative to installed or captured values.
    pub dirty: BTreeMap<String, Vec<String>>,
    /// Complete published history in model order.
    pub log: Vec<BTreeMap<String, String>>,
    /// Rogue-write marker retained for strict schema validation.
    #[serde(rename = "rogue")]
    _rogue: bool,
    /// Stale-base witness marker.
    #[serde(rename = "staleBaseCommitted")]
    _stale_base_committed: bool,
    /// Actual expected bytes for each live leaf.
    pub view: BTreeMap<String, BTreeMap<String, String>>,
    /// Lease holders, with None denoting an unheld path.
    pub holder: BTreeMap<String, String>,
    /// Per-path model token counters.
    #[serde(rename = "token")]
    _token: BTreeMap<String, u64>,
    /// Abstract merge records retained without deriving expectations from the runtime.
    #[serde(rename = "merges")]
    _merges: Vec<serde_json::Value>,
    /// Captured model proposals.
    pub proposals: Vec<Proposal>,
    /// Expiry witness marker.
    #[serde(rename = "expired")]
    _expired: bool,
    /// Stale paths for each symbolic leaf.
    #[serde(rename = "stale")]
    _stale: BTreeMap<String, Vec<String>>,
    /// Last observed model epoch for each leaf.
    #[serde(rename = "readEpoch")]
    _read_epoch: BTreeMap<String, u64>,
    /// Captured model reservations.
    #[serde(rename = "held")]
    _held: BTreeMap<String, BTreeMap<String, u64>>,
    /// Optional deterministic-wrapper position.
    #[serde(default, rename = "step")]
    _step: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Counterexample {
    /// TLC's native action descriptions, retained to verify trace length.
    action: Vec<Vec<serde_json::Value>>,
    /// Numbered post-states supplied by TLC.
    pub state: Vec<(u64, State)>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Export {
    /// Checked state/action history.
    pub counterexample: Counterexample,
    /// Model variable names in the native export.
    #[serde(rename = "vars")]
    _vars: Vec<String>,
}

pub fn checked(root: &Path, name: &str) -> Result<(Manifest, Export)> {
    let directory = root.join("tests/traces");
    let manifest: Manifest =
        serde_json::from_slice(&fs::read(directory.join(format!("{name}.manifest.json")))?)?;
    if manifest.schema_version != 1
        || manifest.checker_version != "1.8.0"
        || manifest.checker_sha256
            != "066cd246d87a388dfde0f04c3b506007f4c0cb4708a5b5396f0552a005eb75b5"
    {
        return Err("trace checker identity changed".into());
    }
    for (path, expected) in &manifest.inputs {
        if !path.starts_with("specs/")
            || Path::new(path).components().any(|part| part == std::path::Component::ParentDir)
        {
            return Err("invalid model input path".into());
        }
        if digest(&fs::read(root.join(path))?) != *expected {
            return Err(format!("model input changed: {path}").into());
        }
    }
    let bytes = fs::read(directory.join(format!("{name}.json")))?;
    if digest(&bytes) != manifest.export_sha256 {
        return Err("TLC export changed".into());
    }
    let exported: Export = serde_json::from_slice(&bytes)?;
    if exported.counterexample.state.len() != manifest.actions.len() + 1
        || exported.counterexample.action.len() != manifest.actions.len()
    {
        return Err("trace/action length mismatch".into());
    }
    for step in &manifest.actions {
        if step.leaf.is_empty()
            || !step.leaf.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err("invalid symbolic leaf".into());
        }
        cowtree_metadata::ResourcePath::parse(&step.path)?;
    }
    Ok((manifest, exported))
}

pub fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
