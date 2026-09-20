// Copyright (c) 2026 Windsor Nguyen

//! Snapshot entries separate captured attributes from source inode identity.

use crate::PathClass;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TreeKind {
    File,
    Directory,
    Symlink,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileIdentity {
    pub device: u64,
    pub inode: u64,
    /// Signed nanoseconds preserve every timestamp representable by Unix stat.
    pub changed_ns: i128,
}

#[derive(Debug, Clone, Serialize)]
pub struct TreeEntry {
    #[serde(serialize_with = "crate::path_json::path")]
    pub path: PathBuf,
    pub kind: TreeKind,
    pub classification: PathClass,
    pub mode: u32,
    pub size: u64,
    pub mtime_ns: i128,
    pub digest: Option<String>,
    #[serde(serialize_with = "crate::path_json::optional_path")]
    pub link: Option<PathBuf>,
    pub identity: Option<FileIdentity>,
}

impl PartialEq for TreeEntry {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
            && self.kind == other.kind
            && self.classification == other.classification
            && self.mode == other.mode
            && self.size == other.size
            && self.mtime_ns == other.mtime_ns
            && self.digest == other.digest
            && self.link == other.link
    }
}

impl Eq for TreeEntry {}
