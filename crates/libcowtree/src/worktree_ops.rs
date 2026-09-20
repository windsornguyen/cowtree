// Copyright (c) 2026 Windsor Nguyen

//! Standalone inspection and retirement share Git's common repository lock.

use crate::{Worktree, WorktreeError as Error, WorktreeResult as Result, git::Git};
use std::path::Path;

pub fn list_worktrees(source: Option<&Path>) -> Result<Vec<Worktree>> {
    let repository = Git::discover(source)?;
    let _lock = repository.lock()?;
    repository.worktrees()
}

/// Remove a worktree through Git's dirty-file and lock checks without deleting refs.
pub fn remove_worktree(path: &Path, source: Option<&Path>, force: bool) -> Result<()> {
    let path = std::path::absolute(path).map_err(|error| Error::io(path, error))?;
    let repository = Git::discover(source)?;
    let _lock = repository.lock()?;
    let mut command = repository.command();
    command.args(["worktree", "remove"]);
    if force {
        command.arg("--force");
    }
    repository.capture(command.arg("--").arg(path))?;
    Ok(())
}
