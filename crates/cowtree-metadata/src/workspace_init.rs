// Copyright (c) 2026 Windsor Nguyen

//! Publish the authority and its Python workspace configuration under one durable name.
//!
//! Initialization happens in a private sibling directory. The destination becomes
//! visible only after SQLite and configuration are durable, and never replaces an
//! existing destination. A pre-publication crash leaves only an inspectable staging
//! directory; a post-publication crash leaves a complete authority ready for import.

use crate::{Error, Limits, Result, Store, Version, durability, tree_io};
use serde::Serialize;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Serialize)]
struct WorkspaceConfig<'a> {
    /// Version of the Python workspace configuration schema.
    format: u8,
    /// Canonical source checkout path used to resume import.
    source: PathBuf,
    /// Full Git object identity of the source checkout.
    commit: &'a str,
    /// Import remains pending until its snapshot publication completes.
    initial_version: Option<Version>,
}

impl Store {
    /// Atomically expose a new metadata authority together with its workspace config.
    pub fn create_workspace(
        root: &Path,
        source: &Path,
        commit: &str,
        limits: Limits,
    ) -> Result<Self> {
        if !matches!(commit.len(), 40 | 64) || !commit.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(Error::InstallConflict(
                "commit must be a full hexadecimal Git object identity".into(),
            ));
        }
        let source = fs::canonicalize(source).map_err(|error| tree_io::io(source, error))?;
        tree_io::identity(&source)?;
        let name =
            root.file_name().ok_or_else(|| Error::InstallConflict(root.display().to_string()))?;
        let parent =
            root.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let parent = fs::canonicalize(parent).map_err(|error| tree_io::io(parent, error))?;
        let destination = parent.join(name);
        let staging = tempfile::Builder::new()
            .prefix(".cowtree-init-")
            .tempdir_in(&parent)
            .map_err(|error| tree_io::io(&parent, error))?;
        let authority = staging.path().join("authority");
        let store = Store::create(&authority, limits)?;
        let config = WorkspaceConfig { format: 1, source, commit, initial_version: None };
        write_config(&authority, &config)?;
        store.connection.close().map_err(|(_, error)| Error::Sqlite(error))?;
        durability::sync_directory(&authority).map_err(|error| tree_io::io(&authority, error))?;
        crate::fault::checkpoint("before-authority-publish");
        rustix::fs::renameat_with(
            rustix::fs::CWD,
            &authority,
            rustix::fs::CWD,
            &destination,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(|error| tree_io::io(&destination, error.into()))?;
        durability::sync_directory(&parent).map_err(|error| tree_io::io(&parent, error))?;
        crate::fault::checkpoint("after-authority-publish");
        Store::open(&destination)
    }
}

fn write_config(authority: &Path, config: &WorkspaceConfig<'_>) -> Result<()> {
    let path = authority.join("workspace.json");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| tree_io::io(&path, error))?;
    file.write_all(&serde_json::to_vec(config)?).map_err(|error| tree_io::io(&path, error))?;
    durability::sync_file(&file).map_err(|error| tree_io::io(&path, error))?;
    Ok(())
}
