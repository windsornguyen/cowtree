// Copyright (c) 2026 Windsor Nguyen

//! Validate one union of disjoint captures before publishing its members atomically.

use std::collections::BTreeSet;

use cowtree_metadata::{BatchCandidate, BatchReceipt, LeafId};

use crate::{CheckRequest, Leaf, Result, Validation, Workspace, error::Issue, session::Session};

impl Workspace {
    pub fn prepare_batch(&self, identities: &[LeafId]) -> Result<BatchCandidate> {
        let unique: BTreeSet<_> = identities.iter().copied().collect();
        if unique.is_empty() || unique.len() != identities.len() {
            return Err(Issue::InvalidCandidate.at(&self.root));
        }
        let mut session = self.lock()?;
        let mut selected: Vec<_> =
            unique.into_iter().map(|id| session.read_leaf(id)).collect::<Result<_>>()?;
        let requests = selected
            .iter()
            .map(|leaf| {
                leaf.pending
                    .as_ref()
                    .filter(|pending| pending.submitted && !pending.aborting)
                    .map(|pending| pending.request)
                    .ok_or_else(|| Issue::PendingPublication.at(&leaf.path))
            })
            .collect::<Result<Vec<_>>>()?;
        let parent = session.nodes().read(&session.workspace.config.warm_tip)?;
        let (tip, manifest) = session.published()?;
        if parent.source != manifest {
            return Err(Issue::NodeChanged.at(&self.root));
        }
        let candidate = session.authority.prepare_batch(requests, tip)?;
        if selected.len() != candidate.members.len() {
            return Err(Issue::InvalidCandidate.at(&self.root));
        }
        for (leaf, member) in selected.iter_mut().zip(&candidate.members) {
            let pending =
                leaf.pending.as_mut().ok_or_else(|| Issue::IncompleteRecord.at(&leaf.path))?;
            pending.candidate = Some(member.clone());
            pending.batch = Some(candidate.clone());
            pending.parent_node = Some(parent.id.clone());
            pending.validation = None;
            session.save_leaf(leaf)?;
        }
        Ok(candidate)
    }

    pub fn check_batch(
        &self,
        candidate: &BatchCandidate,
        command: Vec<String>,
        timeout_seconds: u64,
        supervisor: std::path::PathBuf,
    ) -> Result<Validation> {
        self.lock()?.batch_members(candidate)?;
        let first =
            candidate.members.first().ok_or_else(|| Issue::InvalidCandidate.at(&self.root))?;
        let validation = self.check(&CheckRequest {
            supervisor,
            identity: first.request.leaf,
            command,
            timeout_seconds,
        })?;
        let session = self.lock()?;
        let selected = session.batch_members(candidate)?;
        if validation.candidate != *first {
            return Err(Issue::InvalidCandidate.at(&self.root));
        }
        for (mut leaf, member) in selected.into_iter().zip(&candidate.members) {
            let pending =
                leaf.pending.as_mut().ok_or_else(|| Issue::IncompleteRecord.at(&leaf.path))?;
            let mut checked = validation.clone();
            checked.candidate = member.clone();
            pending.validation = Some(checked);
            session.save_leaf(&leaf)?;
        }
        Ok(validation)
    }

    pub fn commit_batch(&self, candidate: &BatchCandidate) -> Result<BatchReceipt> {
        if candidate.members.is_empty() {
            return Err(Issue::InvalidCandidate.at(&self.root));
        }
        let mut session = self.lock()?;
        let receipts = candidate
            .members
            .iter()
            .map(|member| session.authority.result(member.request))
            .collect::<cowtree_metadata::Result<Vec<_>>>()?;
        if receipts.iter().any(Option::is_some) {
            if receipts.iter().any(Option::is_none) {
                return Err(Issue::IncompleteRecord.at(&self.root));
            }
            return Ok(session.authority.commit_batch(candidate.clone())?);
        }
        let selected = session.batch_members(candidate)?;
        let validation = selected
            .first()
            .and_then(|leaf| leaf.pending.as_ref())
            .and_then(|pending| pending.validation.as_ref())
            .ok_or_else(|| Issue::InvalidCandidate.at(&self.root))?;
        for (leaf, member) in selected.iter().zip(&candidate.members) {
            let mut expected = validation.clone();
            expected.candidate = member.clone();
            if leaf.pending.as_ref().and_then(|pending| pending.validation.as_ref())
                != Some(&expected)
            {
                return Err(Issue::InvalidCandidate.at(&leaf.path));
            }
        }
        let node = session.nodes().read(&validation.node)?;
        session.nodes().verify(&node)?;
        if node.source != session.candidate_manifest(&validation.candidate)? {
            return Err(Issue::NodeChanged.at(&self.root));
        }
        let receipt = session.authority.commit_batch(candidate.clone())?;
        crate::fault::checkpoint("after-batch-commit");
        if receipt.receipts.len() != selected.len() {
            return Err(Issue::IncompleteRecord.at(&self.root));
        }
        for (leaf, member) in selected.into_iter().zip(&receipt.receipts) {
            session.finish_publication(leaf, member)?;
        }
        session.pin_origins()?;
        Ok(receipt)
    }
}

impl Session {
    fn batch_members(&self, candidate: &BatchCandidate) -> Result<Vec<Leaf>> {
        let identities: Vec<_> =
            candidate.members.iter().map(|member| member.request.leaf).collect();
        let unique: BTreeSet<_> = identities.iter().copied().collect();
        if identities.is_empty() || identities != unique.into_iter().collect::<Vec<_>>() {
            return Err(Issue::InvalidCandidate.at(&self.workspace.root));
        }
        let selected =
            identities.into_iter().map(|id| self.read_leaf(id)).collect::<Result<Vec<_>>>()?;
        for (leaf, member) in selected.iter().zip(&candidate.members) {
            if !leaf.pending.as_ref().is_some_and(|pending| {
                !pending.aborting
                    && pending.candidate.as_ref() == Some(member)
                    && pending.batch.as_ref() == Some(candidate)
            }) {
                return Err(Issue::InvalidCandidate.at(&leaf.path));
            }
        }
        Ok(selected)
    }
}
