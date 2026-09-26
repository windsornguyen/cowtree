// Copyright (c) 2026 Windsor Nguyen

//! Read immutable files and gitlinks from the selected commit.

use crate::git::Git;
use crate::{FileMode, GitField, TrackedFile, WorktreeError as Error, WorktreeResult as Result};
use cowtree_git::decode_path;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub(crate) struct Snapshot {
    /// Immutable selected superproject commit.
    pub commit: String,
    /// Ordinary tracked files eligible for native cloning.
    pub entries: Vec<TrackedFile>,
    /// Child repository roots, not regular files.
    pub submodules: Vec<Pin>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Pin {
    /// Path within the containing repository.
    pub path: PathBuf,
    /// Commit recorded by the gitlink.
    pub commit: String,
}

impl Git {
    pub(crate) fn tree(&self, commit: String) -> Result<Snapshot> {
        let data = self.capture(self.command()?.args(["ls-tree", "-r", "-z"]).arg(&commit))?;
        let mut snapshot = Snapshot { commit, entries: Vec::new(), submodules: Vec::new() };
        for record in data.split(|byte| *byte == 0).filter(|record| !record.is_empty()) {
            let tab = record
                .iter()
                .position(|byte| *byte == b'\t')
                .ok_or(Error::GitResponse { field: GitField::TreeEntry })?;
            let path = decode_path(record[tab + 1..].to_vec())?;
            let fields: Vec<_> = record[..tab].split(|byte| *byte == b' ').collect();
            let [mode, _, object] = fields.as_slice() else {
                return Err(Error::GitResponse { field: GitField::TreeEntry });
            };
            let mode = match *mode {
                b"100644" => FileMode::Regular,
                b"100755" => FileMode::Executable,
                b"120000" => FileMode::Symlink,
                b"160000" => {
                    TrackedFile::new(path.clone(), FileMode::Regular)?;
                    let commit = std::str::from_utf8(object)
                        .map_err(|_| Error::GitResponse { field: GitField::Head })?
                        .to_owned();
                    if !matches!(commit.len(), 40 | 64)
                        || !commit.bytes().all(|b| b.is_ascii_hexdigit())
                    {
                        return Err(Error::GitResponse { field: GitField::Head });
                    }
                    snapshot.submodules.push(Pin { path, commit });
                    continue;
                }
                _ => return Err(Error::UnsupportedMode { path }),
            };
            snapshot.entries.push(TrackedFile::new(path, mode)?);
        }
        Ok(snapshot)
    }
}
