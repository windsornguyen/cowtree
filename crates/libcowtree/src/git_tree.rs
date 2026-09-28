// Copyright (c) 2026 Windsor Nguyen

//! Read immutable files and gitlinks from the selected commit.

use crate::git::Git;
use crate::{FileMode, GitField, TrackedFile, WorktreeError as Error, WorktreeResult as Result};
use cowtree_git::{checked, decode_path};
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
        self.fetch_missing_blobs(&commit)?;
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

    /// Fetch the blobs of `commit` that a partial clone lacks, in one request.
    ///
    /// A blobless clone holds no blob it never checked out, and Git's lazy fetch asks for
    /// one missing blob per round trip when the worktree's files are written. That took over
    /// seven minutes on a 15,000-file repository. This is the request `git checkout` makes
    /// for the same blobs. A blob is missing only when no checkout ever wrote it, so these
    /// are the files this worktree writes rather than clones.
    fn fetch_missing_blobs(&self, commit: &str) -> Result<()> {
        let Some(remote) = self.promisor_remote()? else {
            return Ok(());
        };
        let listed = self.capture(
            self.command()?
                .args(["rev-list", "--objects", "--no-walk", "--missing=print"])
                .arg(commit),
        )?;
        let mut wanted = Vec::new();
        for line in listed.split(|byte| *byte == b'\n') {
            if let Some(object) = line.strip_prefix(b"?") {
                wanted.extend_from_slice(object);
                wanted.push(b'\n');
            }
        }
        if wanted.is_empty() {
            return Ok(());
        }
        self.input(
            self.command()?
                .args(["-c", "fetch.negotiationAlgorithm=noop", "fetch", "--no-tags"])
                .args(["--no-write-fetch-head", "--recurse-submodules=no"])
                .args(["--filter=blob:none", "--stdin"])
                .arg(&remote.0),
            &wanted,
        )?;
        Ok(())
    }

    /// The remote a partial clone fetches its missing objects from, if this is one.
    fn promisor_remote(&self) -> Result<Option<Remote>> {
        let output = self
            .command()?
            .args(["config", "--get-regexp", r"^remote\..+\.promisor$"])
            .output()
            .map_err(|error| Error::io(&self.root, error))?;
        if output.status.code() == Some(1) {
            return Ok(None);
        }
        let listed = checked(output)?;
        let remote = listed.split(|byte| *byte == b'\n').find_map(|line| {
            let setting = line.strip_prefix(b"remote.")?;
            let name = setting.strip_suffix(b".promisor true")?;
            String::from_utf8(name.to_vec()).ok().map(Remote)
        });
        Ok(remote)
    }
}

/// Name of a Git remote, as `git remote` lists it.
struct Remote(String);
