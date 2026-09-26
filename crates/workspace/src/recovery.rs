// Copyright (c) 2026 Windsor Nguyen

//! Reconcile durable intents in dependency order before admitting another transition.

use std::{
    fs::{self, File},
    os::unix::fs::MetadataExt,
    path::Path,
    sync::Arc,
};

use cowtree_metadata::{LeafId, Store};

use crate::{
    Config, Error, Node, Result, Workspace, durability,
    error::Issue,
    git::Repository,
    imports, paths, records,
    session::Session,
    workspace::{Initialization, canonical_target},
};

impl Session {
    pub fn reconcile(&mut self) -> Result<Vec<LeafId>> {
        self.recover_views()?;
        self.recover_seals()?;
        let recovered = self.recover_forks()?;
        self.recover_captures()?;
        self.recover_publications()?;
        self.recover_drops()?;
        self.recover_collection()?;
        Ok(recovered)
    }
}

impl Workspace {
    pub fn recover(&self) -> Result<Vec<LeafId>> {
        let mut session = self.lock_raw()?;
        let recovered = session.reconcile()?;
        session.pin_origins()?;
        Ok(recovered)
    }

    pub fn recover_initialization(root: &Path) -> Result<bool> {
        if root.is_symlink() {
            return Err(Issue::InvalidRoot.at(root));
        }
        let root = canonical_target(root)?;
        if root.exists() {
            Self::open(&root)?;
            return Ok(true);
        }
        let staging = Self::staging(&root)?;
        let lock_path = staging.join("lock");
        let lock = match File::options().read(true).write(true).open(&lock_path) {
            Ok(lock) => lock,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if root.exists() {
                    Self::open(&root)?;
                    return Ok(true);
                }
                if staging.exists() {
                    return Err(Issue::IncompleteRecord.at(&staging));
                }
                return Ok(false);
            }
            Err(error) => return Err(Error::io(&lock_path, error)),
        };
        fs4::FileExt::lock(&lock).map_err(|error| Error::io(&lock_path, error))?;
        if root.exists() {
            Self::open(&root)?;
            return Ok(true);
        }
        if !staging.exists() {
            return Ok(false);
        }
        let held = lock.metadata().map_err(|error| Error::io(&lock_path, error))?;
        let current = fs::metadata(&lock_path).map_err(|error| Error::io(&lock_path, error))?;
        if (held.dev(), held.ino()) != (current.dev(), current.ino()) {
            return Err(Issue::ChangedDirectory.at(&staging));
        }
        let record: Initialization = records::read(&staging.join("initialization.json"))?;
        if record.target != root {
            return Err(Issue::ChangedDirectory.at(&staging));
        }
        let lock = Arc::new(lock);
        if staging.join("import.json").exists() {
            let config: Config = records::read(&staging.join("import.json"))?;
            if config.location != root {
                return Err(Issue::ChangedDirectory.at(&staging));
            }
            imports::run(&staging, &config, vec![lock.clone()])?;
        }
        if staging.join("workspace.json").exists() {
            Store::open(&staging.join("authority"))?.tip()?;
            durability::publish_directory(&staging, &root)?;
            return Ok(true);
        }
        discard_initialization(&staging, record, lock)?;
        Ok(false)
    }

    pub fn list(&self) -> Result<Vec<crate::Leaf>> {
        self.lock()?.leaves()
    }

    pub fn log(&self) -> Result<Vec<Node>> {
        let session = self.lock()?;
        let mut nodes = Vec::new();
        for path in paths::entries(&self.root.join("nodes"))? {
            if !path.join("node.json").exists() {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| Issue::NonUtf8.at(&path))?;
            nodes.push(session.nodes().read(&crate::NodeId::parse(name)?)?);
        }
        Ok(nodes)
    }
}

fn discard_initialization(staging: &Path, record: Initialization, lock: Arc<File>) -> Result<()> {
    let repository = Repository::authority(&record.git_directory, vec![lock]);
    if staging.join("nodes").exists() {
        for path in paths::entries(&staging.join("nodes"))? {
            if !path.join("node.json").exists() {
                continue;
            }
            let node: Node = records::read(&path.join("node.json"))?;
            if path.file_name().and_then(|name| name.to_str()) != Some(node.id.as_str()) {
                return Err(Issue::NodeIdentity.at(&path));
            }
            repository.remove_ref(
                &format!("refs/cowtree/nodes/{}", node.id.as_str()),
                &node.git_commit,
            )?;
            repository
                .remove_ref(&format!("refs/cowtree/tips/{}", node.id.as_str()), &node.git_commit)?;
        }
    }
    records::remove_directory(staging)?;
    Ok(())
}
