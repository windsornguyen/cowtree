// Copyright (c) 2026 Windsor Nguyen

//! Capture durable checkpoints before publishing private refs.

use std::{
    fs,
    path::{Path, PathBuf},
};

use cowtree::{CaptureMode, TreeKind};
use cowtree_metadata::Snapshot;
use unicode_normalization::UnicodeNormalization;

use crate::{
    CommitId, Error, Node, NodeId, Policy, Result, durability, error::Issue, git::Repository,
    manifest, paths, records, submodules,
};

pub(crate) struct Nodes {
    /// Owned immutable checkpoint directory.
    pub directory: PathBuf,
    /// Repository and locks used to publish private refs.
    pub repository: Repository,
    /// Workspace-wide capture rules.
    pub policy: Policy,
}

#[derive(Default)]
pub(crate) struct Seal {
    /// Published origins carried forward from the leaf.
    pub origins: Option<Snapshot>,
    /// Previous private checkpoint, when extending a leaf history.
    pub parent: Option<NodeId>,
    /// Git ancestry used for the initial capture.
    pub git_parent: Option<CommitId>,
    /// Already verified candidate commit to reuse exactly.
    pub reuse: Option<CommitId>,
    /// Journal-reserved identity for replayable checkpoint creation.
    pub identity: Option<NodeId>,
}

impl Nodes {
    pub fn read(&self, id: &NodeId) -> Result<Node> {
        let node: Node = records::read(&self.directory.join(id.as_str()).join("node.json"))?;
        if node.id != *id {
            return Err(Issue::NodeIdentity.at(&self.directory));
        }
        Ok(node)
    }

    pub fn seal(&self, source: &Path, options: Seal) -> Result<Node> {
        if paths::entries(&self.directory)?.len() >= 256 {
            return Err(Issue::SnapshotLimit.at(&self.directory));
        }
        let identity = options.identity.clone().unwrap_or_else(NodeId::generate);
        let directory = self.directory.join(identity.as_str());
        fs::create_dir(&directory).map_err(|error| Error::io(&directory, error))?;
        let result = self.capture(source, identity, &options);
        let node = match result {
            Ok(node) => node,
            Err(original) => {
                if let Err(cleanup) = records::remove_directory(&directory) {
                    return Err(Error::Cleanup {
                        original: Box::new(original),
                        cleanup: Box::new(cleanup),
                    });
                }
                return Err(original);
            }
        };
        Ok(node)
    }

    pub fn publish(&self, node: &Node) -> Result<()> {
        // Git filters may mutate the captured image.
        self.verify(node)?;
        self.repository.set_ref(
            &format!("refs/cowtree/nodes/{}", node.id.as_str()),
            &node.git_commit,
            None,
        )
    }

    fn capture(&self, source: &Path, identity: NodeId, options: &Seal) -> Result<Node> {
        let directory = self.directory.join(identity.as_str());
        let tree = directory.join("tree");
        let policy = self.policy.working(source)?;
        let native = policy.compile()?;
        let mut entries = cowtree::clone_tree(source, &tree, &native)?;
        if options.parent.is_none() && !policy.pins.is_empty() {
            submodules::materialize(&self.repository, source, &tree, &policy)?;
            entries = cowtree::scan_tree(&tree, &native, CaptureMode::Content)?;
        }
        let names: Vec<String> = entries
            .iter()
            .map(|entry| {
                entry
                    .path
                    .to_str()
                    .ok_or_else(|| Issue::NonUtf8.at(&entry.path))
                    .map(|name| name.nfc().collect())
            })
            .collect::<Result<_>>()?;
        paths::aliases(
            names
                .iter()
                .chain(&self.policy.derived)
                .chain(&self.policy.ephemeral)
                .map(String::as_str),
        )?;
        let source = manifest::capture(&tree, &entries, &policy)?;
        let parent = options.parent.as_ref().map(|id| self.read(id)).transpose()?;
        if let Some(parent) = &parent {
            manifest::verify_dependencies(&source, &parent.source)?;
        }
        let parents: Vec<_> = parent
            .as_ref()
            .map(|node| node.git_commit.clone())
            .or_else(|| options.git_parent.clone())
            .into_iter()
            .collect();
        let commit = self.repository.project(&tree, &source, &parents, options.reuse.as_ref())?;
        let files: Vec<_> = entries
            .iter()
            .filter(|entry| entry.kind == TreeKind::File)
            .map(|entry| tree.join(&entry.path))
            .collect();
        let mut directories: Vec<_> = entries
            .iter()
            .rev()
            .filter(|entry| entry.kind == TreeKind::Directory)
            .map(|entry| tree.join(&entry.path))
            .collect();
        directories.extend([tree, directory.clone()]);
        durability::sync_tree(
            &self.directory,
            files.iter().map(PathBuf::as_path),
            directories.iter().map(PathBuf::as_path),
        )?;
        let origins = options.origins.clone().unwrap_or_else(|| source.clone());
        let node = Node {
            id: identity,
            parent: options.parent.clone(),
            source,
            origins,
            git_commit: commit,
            policy,
        };
        records::write(&directory.join("node.json"), &node)?;
        crate::fault::checkpoint("after-node-record");
        records::sync_directory(&self.directory)?;
        Ok(node)
    }

    pub fn verify(&self, node: &Node) -> Result<()> {
        let tree = self.directory.join(node.id.as_str()).join("tree");
        let entries = cowtree::scan_tree(&tree, &node.policy.compile()?, CaptureMode::Content)?;
        if manifest::capture(&tree, &entries, &node.policy)? != node.source {
            return Err(Issue::NodeChanged.at(&tree));
        }
        Ok(())
    }
}
