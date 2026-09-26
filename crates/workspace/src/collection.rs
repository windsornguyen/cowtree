// Copyright (c) 2026 Windsor Nguyen

//! Quarantine unreachable images while live operation records pin their inputs.

use std::{collections::BTreeSet, fs};

use cowtree_metadata::{LeafId, Maintenance};
use serde::{Deserialize, Serialize};

use crate::{
    CommitId, Error, Leaf, NodeId, Result, Workspace, durability,
    error::Issue,
    leaves::ForkRecord,
    paths, records,
    seals::{RetainedNode, SealRecord},
    session::Session,
    types::PublicationRecord,
    views::ViewRecord,
};

#[derive(Serialize)]
pub struct Collection {
    /// Checkpoint images retired by this pass.
    pub nodes: Vec<NodeId>,
    /// Inactive validation worktrees retired by this pass.
    pub checks: Vec<LeafId>,
    /// Independent authority reclamation receipt.
    pub metadata: Maintenance,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct QuarantinedNode {
    /// Checkpoint whose directory has been made ineligible for new readers.
    node: NodeId,
    /// Exact private ref target when a complete node exists.
    commit: Option<CommitId>,
}

impl Workspace {
    pub fn collect(&self) -> Result<Collection> {
        let mut session = self.lock()?;
        let mut checks = Vec::new();
        for leaf in session.leaves()? {
            if leaf.check_candidate.is_none() {
                continue;
            }
            match session.begin_drop(leaf.clone()) {
                Ok(()) => checks.push(leaf.id),
                Err(Error::State { issue: Issue::ValidationBusy, .. }) => {}
                Err(error) => return Err(error),
            }
        }
        let protected = session.roots()?;
        let mut removed = Vec::new();
        for directory in paths::entries(&self.root.join("nodes"))? {
            let name = directory
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| Issue::NonUtf8.at(&directory))?;
            let node = NodeId::parse(name)?;
            if protected.contains(&node) {
                continue;
            }
            let commit = if directory.join("node.json").exists() {
                Some(session.nodes().read(&node)?.git_commit)
            } else {
                None
            };
            records::write(
                &self.root.join("trash").join(format!("{}.json", node.as_str())),
                &QuarantinedNode { node: node.clone(), commit },
            )?;
            crate::fault::checkpoint("after-quarantine-intent");
            removed.push(node);
        }
        session.recover_collection()?;
        session.collect_artifacts()?;
        session.pin_origins()?;
        Ok(Collection { nodes: removed, checks, metadata: session.authority.maintain()? })
    }
}

impl Session {
    pub fn recover_collection(&self) -> Result<()> {
        let markers: Vec<_> = paths::entries(&self.workspace.root.join("trash"))?
            .into_iter()
            .filter(|path| path.extension().is_some_and(|extension| extension == "json"))
            .collect();
        if markers.is_empty() {
            return Ok(());
        }
        let protected = self.roots()?;
        for marker in markers {
            let node: QuarantinedNode = records::read(&marker)?;
            if marker.file_stem().and_then(|name| name.to_str()) != Some(node.node.as_str()) {
                return Err(Issue::NodeIdentity.at(&marker));
            }
            if protected.contains(&node.node) {
                return Err(Issue::IncompleteRecord.at(&marker));
            }
            let source = self.workspace.root.join("nodes").join(node.node.as_str());
            let target = self.workspace.root.join("trash").join(node.node.as_str());
            if source.exists() {
                durability::publish_directory(&source, &target)?;
            }
            if let Some(commit) = node.commit {
                self.repository()
                    .remove_ref(&format!("refs/cowtree/nodes/{}", node.node.as_str()), &commit)?;
            }
            if target.exists() {
                records::remove_directory(&target)?;
            }
            fs::remove_file(&marker).map_err(|error| Error::io(&marker, error))?;
            records::sync_directory(&self.workspace.root.join("trash"))?;
        }
        Ok(())
    }

    fn roots(&self) -> Result<BTreeSet<NodeId>> {
        let mut roots = BTreeSet::from([
            self.workspace.config.initial.clone(),
            self.workspace.config.warm_tip.clone(),
        ]);
        for leaf in self.leaves()? {
            leaf_roots(&leaf, &mut roots);
        }
        for path in paths::entries(&self.workspace.root.join("retained"))? {
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let retained: RetainedNode = records::read(&path)?;
            if path.file_stem().and_then(|name| name.to_str()) != Some(retained.node.as_str()) {
                return Err(Issue::NodeIdentity.at(&path));
            }
            roots.insert(retained.node);
        }
        for directory in paths::entries(&self.workspace.root.join("operations"))? {
            if directory.join("fork.json").exists() {
                let record: ForkRecord = records::read(&directory.join("fork.json"))?;
                roots.insert(record.node);
            } else if directory.join("seal.json").exists() {
                let record: SealRecord = records::read(&directory.join("seal.json"))?;
                roots.extend([record.node, record.before.node]);
            } else if directory.join("view.json").exists() {
                let record: ViewRecord = records::read(&directory.join("view.json"))?;
                leaf_roots(&record.before, &mut roots);
                if let Some(after) = record.after {
                    leaf_roots(&after, &mut roots);
                }
            } else {
                return Err(Issue::UnknownOperation.at(&directory));
            }
        }
        Ok(roots)
    }

    fn collect_artifacts(&self) -> Result<()> {
        let root = &self.workspace.root;
        let mut receipts = Vec::new();
        for path in paths::entries(&root.join("receipts"))? {
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let version = path
                .file_stem()
                .and_then(|name| name.to_str())
                .and_then(|name| name.parse::<u64>().ok())
                .ok_or_else(|| Issue::IncompleteRecord.at(&path))?;
            receipts.push((version, path));
        }
        receipts.sort_by_key(|(version, _)| std::cmp::Reverse(*version));
        let mut protected = BTreeSet::new();
        for (_, path) in receipts.iter().take(128) {
            let record: PublicationRecord = records::read(path)?;
            protected.insert(record.validation.log);
        }
        for (_, path) in receipts.iter().skip(128) {
            fs::remove_file(path).map_err(|error| Error::io(path, error))?;
        }
        let leaves = self.leaves()?;
        for leaf in &leaves {
            if leaf.check_candidate.is_some() {
                protected.insert(root.join("checks").join(format!("{}.log", leaf.id.get())));
            }
            if let Some(validation) =
                leaf.pending.as_ref().and_then(|pending| pending.validation.as_ref())
            {
                protected.insert(validation.log.clone());
            }
        }
        let mut logs = Vec::new();
        for path in paths::entries(&root.join("checks"))? {
            if path.extension().is_none_or(|extension| extension != "log") {
                continue;
            }
            let modified = fs::metadata(&path)
                .and_then(|metadata| metadata.modified())
                .map_err(|error| Error::io(&path, error))?;
            logs.push((modified, path));
        }
        logs.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
        for (_, path) in logs.iter().skip(64) {
            if !protected.contains(path) {
                fs::remove_file(path).map_err(|error| Error::io(path, error))?;
            }
        }
        let live: BTreeSet<_> = leaves.iter().map(|leaf| leaf.id.get()).collect();
        for path in paths::entries(&root.join("checks"))? {
            let Some(name) = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_suffix("-install"))
            else {
                continue;
            };
            let id = name.parse::<u64>().map_err(|_| Issue::IncompleteRecord.at(&path))?;
            if !live.contains(&id) {
                records::remove_directory(&path)?;
            }
        }
        records::sync_directory(&root.join("receipts"))?;
        records::sync_directory(&root.join("checks"))
    }
}

fn leaf_roots(leaf: &Leaf, roots: &mut BTreeSet<NodeId>) {
    roots.insert(leaf.node.clone());
    if let Some(pending) = &leaf.pending {
        roots.insert(pending.node.clone());
        roots.extend(pending.parent_node.iter().cloned());
        if let Some(validation) = &pending.validation {
            roots.insert(validation.node.clone());
        }
    }
}
