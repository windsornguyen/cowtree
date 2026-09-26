// Copyright (c) 2026 Windsor Nguyen

//! Freeze publication intent once and replay submission under the same request identity.

use std::{collections::BTreeMap, fs, os::unix::ffi::OsStrExt};

use cowtree_metadata::{FileKind, LeafId, ProposalInput, RequestId};

use crate::{Error, Leaf, Pending, Result, Workspace, error::Issue, session::Session};

impl Workspace {
    pub fn capture(&self, identity: LeafId) -> Result<Option<Pending>> {
        self.lock()?.capture(identity)
    }

    pub fn abort(&self, identity: LeafId) -> Result<()> {
        let mut session = self.lock_raw()?;
        session.recover_views()?;
        session.recover_seals()?;
        session.recover_forks()?;
        session.pin_origins()?;
        let mut leaf = session.read_leaf(identity)?;
        let pending =
            leaf.pending.as_mut().ok_or_else(|| Issue::PendingPublication.at(&leaf.path))?;
        if !pending.aborting {
            let receipt = match session.authority.result(pending.request) {
                Ok(receipt) => receipt,
                Err(cowtree_metadata::Error::RequestExpired(_)) if !pending.submitted => None,
                Err(error) => return Err(error.into()),
            };
            if receipt.is_some() {
                return Err(Issue::InvalidCandidate.at(&leaf.path));
            }
            pending.aborting = true;
            session.save_leaf(&leaf)?;
        }
        session.cancel_capture(leaf)?;
        session.pin_origins()
    }
}

impl Session {
    pub(crate) fn capture(&mut self, identity: LeafId) -> Result<Option<Pending>> {
        let leaf = self.read_leaf(identity)?;
        if leaf.check_candidate.is_some() {
            return Err(Issue::InvalidCandidate.at(&leaf.path));
        }
        if leaf.pending.is_some() {
            return Err(Issue::PendingPublication.at(&leaf.path));
        }
        if self.current(&leaf)? == leaf.origins {
            return Ok(None);
        }
        let node = self.seal(identity, crate::Retention::Referenced)?;
        let selected: std::collections::BTreeSet<_> = node
            .source
            .keys()
            .chain(leaf.origins.keys())
            .filter(|path| node.source.get(*path) != leaf.origins.get(*path))
            .cloned()
            .collect();
        let selected: Vec<_> = selected.into_iter().collect();
        let mut leaf = self.acquire(identity, &selected)?;
        if leaf.node != node.id || leaf.pending.is_some() {
            return Err(Issue::PendingPublication.at(&leaf.path));
        }
        let changes = selected
            .into_iter()
            .filter(|path| node.source.get(path) != leaf.origins.get(path))
            .map(|path| {
                let value = node.source.get(&path).cloned();
                (path, value)
            })
            .collect::<BTreeMap<_, _>>();
        if changes.is_empty() {
            return Ok(None);
        }
        leaf.pending = Some(Pending {
            request: RequestId { leaf: identity, sequence: leaf.sequence.get() },
            node: node.id,
            changes,
            submitted: false,
            aborting: false,
            candidate: None,
            batch: None,
            parent_node: None,
            validation: None,
        });
        self.save_leaf(&leaf)?;
        crate::fault::checkpoint("after-capture-intent");
        let pending = self.submit_capture(leaf)?;
        self.pin_origins()?;
        Ok(Some(pending))
    }

    pub fn submit_capture(&mut self, mut leaf: Leaf) -> Result<Pending> {
        let pending =
            leaf.pending.as_ref().ok_or_else(|| Issue::IncompleteRecord.at(&leaf.path))?;
        let node = self.nodes().read(&pending.node)?;
        self.nodes().verify(&node)?;
        for (path, entry) in &pending.changes {
            let grant =
                leaf.grants.get(path).ok_or_else(|| Issue::IncompleteRecord.at(&leaf.path))?;
            if let Some(entry) = entry {
                let source = self
                    .workspace
                    .root
                    .join("nodes")
                    .join(node.id.as_str())
                    .join("tree")
                    .join(path.as_str());
                let staged = if entry.kind.file_kind() == FileKind::Symlink {
                    let link = fs::read_link(&source).map_err(|error| Error::io(&source, error))?;
                    self.authority.stage(leaf.id, link.as_os_str().as_bytes())?
                } else {
                    self.authority.stage_file(leaf.id, &source)?
                };
                if staged != entry.object {
                    return Err(Issue::SourceChanged.at(&source));
                }
            }
            self.authority.edit(grant, entry.clone())?;
        }
        let proposal = self.authority.propose(ProposalInput {
            request: pending.request,
            paths: pending.changes.keys().cloned().collect(),
        })?;
        let changes: BTreeMap<_, _> =
            proposal.changes.into_iter().map(|change| (change.path, change.value)).collect();
        if changes != pending.changes {
            return Err(Issue::IncompleteRecord.at(&leaf.path));
        }
        let mut submitted = pending.clone();
        submitted.submitted = true;
        leaf.pending = Some(submitted.clone());
        self.save_leaf(&leaf)?;
        Ok(submitted)
    }

    pub fn cancel_capture(&mut self, mut leaf: Leaf) -> Result<()> {
        let pending = leaf
            .pending
            .as_ref()
            .filter(|pending| pending.aborting)
            .ok_or_else(|| Issue::IncompleteRecord.at(&leaf.path))?;
        match self.authority.abort(pending.request) {
            Ok(()) | Err(cowtree_metadata::Error::RequestExpired(_)) => {}
            Err(error) => return Err(error.into()),
        }
        for entry in pending.changes.values().flatten() {
            self.authority.discard_upload(leaf.id, &entry.object)?;
        }
        leaf.sequence = std::num::NonZeroU64::new(
            pending
                .request
                .sequence
                .checked_add(1)
                .ok_or(cowtree_metadata::Error::CounterExhausted)?,
        )
        .ok_or(cowtree_metadata::Error::CounterExhausted)?;
        leaf.pending = None;
        self.save_leaf(&leaf)
    }

    pub fn recover_captures(&mut self) -> Result<()> {
        for leaf in self.leaves()? {
            match &leaf.pending {
                Some(pending) if pending.aborting => self.cancel_capture(leaf)?,
                Some(pending) if !pending.submitted => {
                    self.submit_capture(leaf)?;
                }
                _ => {}
            }
        }
        Ok(())
    }
}
