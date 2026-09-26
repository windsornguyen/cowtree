// Copyright (c) 2026 Windsor Nguyen

//! Serialize workspace transitions while retaining the lock in mutating children.

use std::{
    collections::BTreeSet,
    fs::{self, File},
    os::unix::fs::MetadataExt,
    sync::Arc,
};

use cowtree_metadata::{Store, objects::ObjectId};

use crate::{
    Error, Leaf, NodeId, Result, Workspace, error::Issue, git::Repository, nodes::Nodes, paths,
    records, workspace::DIRECTORIES,
};

pub(crate) struct Session {
    /// Configuration reloaded under the workspace lock.
    pub workspace: Workspace,
    /// One SQLite connection shared by the operation's authority calls.
    pub authority: Store,
    /// Filesystem transaction lock, inherited by mutating subprocesses.
    pub lock: Arc<File>,
}

impl Workspace {
    pub(crate) fn lock(&self) -> Result<Session> {
        let mut session = self.lock_raw()?;
        session.reconcile()?;
        session.pin_origins()?;
        Ok(session)
    }

    pub(crate) fn lock_raw(&self) -> Result<Session> {
        let identity = fs::metadata(&self.root).map_err(|error| Error::io(&self.root, error))?;
        let path = self.root.join("lock");
        let lock = File::options()
            .read(true)
            .append(true)
            .create(true)
            .open(&path)
            .map_err(|error| Error::io(&path, error))?;
        fs4::FileExt::lock(&lock).map_err(|error| Error::io(&path, error))?;
        let current =
            fs::symlink_metadata(&self.root).map_err(|error| Error::io(&self.root, error))?;
        if !current.is_dir() || (identity.dev(), identity.ino()) != (current.dev(), current.ino()) {
            return Err(Issue::ChangedDirectory.at(&self.root));
        }
        for name in DIRECTORIES {
            let path = self.root.join(name);
            if !fs::symlink_metadata(&path).map_err(|error| Error::io(&path, error))?.is_dir() {
                return Err(Issue::InvalidRoot.at(&path));
            }
        }
        let workspace = Self::open(&self.root)?;
        let authority = Store::open(&self.root.join("authority"))?;
        Ok(Session { workspace, authority, lock: Arc::new(lock) })
    }
}

impl Session {
    pub fn repository(&self) -> Repository {
        Repository::authority(&self.workspace.config.git_directory, vec![self.lock.clone()])
    }

    pub fn nodes(&self) -> Nodes {
        Nodes {
            directory: self.workspace.root.join("nodes"),
            repository: self.repository(),
            policy: self.workspace.config.policy.clone(),
        }
    }

    pub fn pin_origins(&mut self) -> Result<()> {
        let mut objects: BTreeSet<ObjectId> = self
            .leaves()?
            .into_iter()
            .flat_map(|leaf| leaf.origins.into_values().map(|entry| entry.object))
            .collect();
        for path in paths::entries(&self.workspace.root.join("nodes"))? {
            if !path.join("node.json").exists() {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| Issue::NonUtf8.at(&path))?;
            let node = self.nodes().read(&NodeId::parse(name)?)?;
            objects.extend(node.origins.into_values().map(|entry| entry.object));
        }
        self.authority.replace_client_pins(&objects)?;
        Ok(())
    }

    pub fn leaves(&self) -> Result<Vec<Leaf>> {
        paths::entries(&self.workspace.root.join("leaves"))?
            .into_iter()
            .filter(|path| path.extension().is_some_and(|suffix| suffix == "json"))
            .map(|path| records::read(&path))
            .collect()
    }

    pub fn read_leaf(&self, identity: cowtree_metadata::LeafId) -> Result<Leaf> {
        let path = self.workspace.root.join("leaves").join(format!("{}.json", identity.get()));
        let leaf: Leaf = records::read(&path)?;
        if leaf.id != identity {
            return Err(Issue::ChangedDirectory.at(&path));
        }
        Ok(leaf)
    }

    pub fn save_leaf(&self, leaf: &Leaf) -> Result<()> {
        records::write(
            &self.workspace.root.join("leaves").join(format!("{}.json", leaf.id.get())),
            leaf,
        )
    }
}
