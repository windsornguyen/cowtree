// Copyright (c) 2026 Windsor Nguyen

//! Retain a private checkpoint before acknowledging the leaf's new Git projection.

use std::{fs, path::Path};

use cowtree_metadata::LeafId;
use serde::{Deserialize, Serialize};

use crate::{
    Error, Leaf, Node, NodeId, Result, Workspace, error::Issue, nodes::Seal, paths, records,
    session::Session,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SealRecord {
    /// Leaf state from which the checkpoint was captured.
    pub before: Leaf,
    /// Exclusively assigned checkpoint directory name.
    pub node: NodeId,
    /// Whether the caller requested a manual retention pin.
    pub retained: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RetainedNode {
    /// Checkpoint retained independently of live descendants.
    pub node: NodeId,
}

#[derive(Clone, Copy)]
pub enum Retention {
    /// Keep the checkpoint until the caller explicitly releases it.
    Manual,
    /// Keep the checkpoint while a live leaf or operation references it.
    Referenced,
}

impl Workspace {
    pub fn seal(&self, identity: LeafId, retention: Retention) -> Result<Node> {
        self.lock()?.seal(identity, retention)
    }

    pub fn retain(&self, identity: &NodeId) -> Result<()> {
        let session = self.lock()?;
        session.nodes().read(identity)?;
        let path = self.root.join("retained").join(format!("{}.json", identity.as_str()));
        if !path.exists() && paths::entries(&self.root.join("retained"))?.len() >= 128 {
            return Err(Issue::SnapshotLimit.at(&self.root));
        }
        records::write(&path, &RetainedNode { node: identity.clone() })
    }

    pub fn release(&self, identity: &NodeId) -> Result<()> {
        let session = self.lock()?;
        session.nodes().read(identity)?;
        let path = self.root.join("retained").join(format!("{}.json", identity.as_str()));
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(Error::io(&path, error)),
        }
        records::sync_directory(&self.root.join("retained"))
    }
}

impl Session {
    pub(crate) fn seal(&mut self, identity: LeafId, retention: Retention) -> Result<Node> {
        let retain = matches!(retention, Retention::Manual);
        let leaf = self.read_leaf(identity)?;
        self.validate_leaf(&leaf)?;
        if retain && paths::entries(&self.workspace.root.join("retained"))?.len() >= 128 {
            return Err(Issue::SnapshotLimit.at(&self.workspace.root));
        }
        let record =
            SealRecord { before: leaf.clone(), node: NodeId::generate(), retained: retain };
        let directory =
            self.workspace.root.join("operations").join(format!("seal-{}", record.node.as_str()));
        fs::create_dir(&directory).map_err(|error| Error::io(&directory, error))?;
        records::write(&directory.join("seal.json"), &record)?;
        let node = self.nodes().seal(
            &leaf.path,
            Seal {
                origins: Some(leaf.origins),
                parent: Some(leaf.node),
                identity: Some(record.node.clone()),
                ..Seal::default()
            },
        )?;
        self.complete_seal(&directory, &record, &node)?;
        self.pin_origins()?;
        Ok(node)
    }

    fn complete_seal(&self, directory: &Path, record: &SealRecord, node: &Node) -> Result<()> {
        if node.parent.as_ref() != Some(&record.before.node) {
            return Err(Issue::NodeIdentity.at(directory));
        }
        let mut updated = record.before.clone();
        updated.node = node.id.clone();
        updated.git_head = node.git_commit.clone();
        let current = self.read_leaf(record.before.id)?;
        if current != record.before && current != updated {
            return Err(Issue::ChangedDirectory.at(&current.path));
        }
        self.nodes().publish(node)?;
        let repository = self.repository().locked()?;
        let target = repository.select(&current.path);
        target.detached(&[record.before.git_head.clone(), node.git_commit.clone()])?;
        target.reset_index(&node.git_commit)?;
        self.save_leaf(&updated)?;
        if record.retained {
            records::write(
                &self.workspace.root.join("retained").join(format!("{}.json", node.id.as_str())),
                &RetainedNode { node: node.id.clone() },
            )?;
        }
        records::remove_directory(directory)
    }

    pub fn recover_seals(&mut self) -> Result<()> {
        for directory in paths::entries(&self.workspace.root.join("operations"))? {
            if !directory
                .file_name()
                .is_some_and(|name| name.as_encoded_bytes().starts_with(b"seal-"))
            {
                continue;
            }
            if !directory.join("seal.json").exists() {
                records::remove_directory(&directory)?;
                continue;
            }
            let record: SealRecord = records::read(&directory.join("seal.json"))?;
            let path = self.workspace.root.join("nodes").join(record.node.as_str());
            if !path.join("node.json").exists() {
                if path.exists() {
                    records::remove_directory(&path)?;
                }
                records::remove_directory(&directory)?;
                continue;
            }
            let node = self.nodes().read(&record.node)?;
            self.complete_seal(&directory, &record, &node)?;
        }
        Ok(())
    }
}
