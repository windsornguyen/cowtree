// Copyright (c) 2026 Windsor Nguyen

//! Publish only a checked candidate and recover its exact authority acknowledgement.

use std::fs;

use cowtree_metadata::{Candidate, Receipt, RequestId, Snapshot, objects::ObjectId};

use crate::{
    Error, Leaf, Result, Workspace, error::Issue, records, session::Session,
    types::PublicationRecord, views::replace_origin,
};

impl Workspace {
    pub fn pending_candidate(&self, identity: cowtree_metadata::LeafId) -> Result<Candidate> {
        let leaf = self.lock()?.read_leaf(identity)?;
        leaf.pending
            .and_then(|pending| pending.candidate)
            .ok_or_else(|| Issue::InvalidCandidate.at(&leaf.path))
    }

    pub fn prepare(&self, identity: cowtree_metadata::LeafId) -> Result<Candidate> {
        let mut session = self.lock()?;
        let mut leaf = session.read_leaf(identity)?;
        let pending = leaf
            .pending
            .as_mut()
            .filter(|pending| pending.submitted && !pending.aborting)
            .ok_or_else(|| Issue::PendingPublication.at(&leaf.path))?;
        let candidate = session.authority.prepare(pending.request)?;
        let parent = session.nodes().read(&session.workspace.config.warm_tip)?;
        let (version, entries) = session.published()?;
        if version != candidate.parent || entries != parent.source {
            return Err(Issue::NodeChanged.at(&self.root));
        }
        pending.candidate = Some(candidate.clone());
        pending.batch = None;
        pending.parent_node = Some(parent.id);
        pending.validation = None;
        session.save_leaf(&leaf)?;
        Ok(candidate)
    }

    pub fn commit(&self, candidate: &Candidate) -> Result<Receipt> {
        let mut session = self.lock()?;
        let leaf = session.read_leaf(candidate.request.leaf)?;
        if leaf.pending.is_none() {
            if !leaf
                .last_receipt
                .as_ref()
                .is_some_and(|receipt| receipt.request == candidate.request)
            {
                return Err(Issue::InvalidCandidate.at(&leaf.path));
            }
            return Ok(session.authority.commit(candidate.clone())?);
        }
        let pending =
            leaf.pending.as_ref().ok_or_else(|| Issue::IncompleteRecord.at(&leaf.path))?;
        let validation = pending
            .validation
            .as_ref()
            .filter(|validation| {
                !pending.aborting
                    && pending.candidate.as_ref() == Some(candidate)
                    && validation.candidate == *candidate
            })
            .ok_or_else(|| Issue::InvalidCandidate.at(&leaf.path))?;
        let node = session.nodes().read(&validation.node)?;
        session.nodes().verify(&node)?;
        if node.source != session.candidate_manifest(candidate)? {
            return Err(Issue::NodeChanged.at(&self.root));
        }
        let receipt = session.authority.commit(candidate.clone())?;
        crate::fault::checkpoint("after-publication-commit");
        session.finish_publication(leaf, &receipt)?;
        session.pin_origins()?;
        Ok(receipt)
    }

    pub fn result(&self, request: RequestId) -> Result<Option<Receipt>> {
        Ok(self.lock()?.authority.result(request)?)
    }
}

impl Session {
    pub(crate) fn candidate_manifest(&self, candidate: &Candidate) -> Result<Snapshot> {
        let path = self.authority.object_directory().join(candidate.root.as_str());
        let bytes = fs::read(&path).map_err(|error| Error::io(&path, error))?;
        if ObjectId::from_bytes(&bytes) != candidate.root {
            return Err(Issue::CorruptObject.at(&path));
        }
        serde_json::from_slice(&bytes).map_err(|error| Error::record(&path, error))
    }

    pub fn finish_publication(&mut self, mut leaf: Leaf, receipt: &Receipt) -> Result<()> {
        let pending =
            leaf.pending.as_ref().ok_or_else(|| Issue::IncompleteRecord.at(&leaf.path))?;
        let validation =
            pending.validation.as_ref().ok_or_else(|| Issue::IncompleteRecord.at(&leaf.path))?;
        let candidate =
            pending.candidate.as_ref().ok_or_else(|| Issue::IncompleteRecord.at(&leaf.path))?;
        let parent_id =
            pending.parent_node.as_ref().ok_or_else(|| Issue::IncompleteRecord.at(&leaf.path))?;
        if receipt.request != pending.request || receipt.root != candidate.root {
            return Err(Issue::InvalidCandidate.at(&leaf.path));
        }
        let (version, entries) = self.published()?;
        if version != receipt.version {
            return Err(Issue::InvalidCandidate.at(&leaf.path));
        }
        let node = self.nodes().read(&validation.node)?;
        if node.source != entries {
            return Err(Issue::NodeChanged.at(&self.workspace.root));
        }
        let parent = self.nodes().read(parent_id)?;
        self.repository().set_ref(
            &format!("refs/cowtree/tips/{}", self.workspace.config.initial.as_str()),
            &node.git_commit,
            Some(&parent.git_commit),
        )?;
        let receipts = self.batch_receipts(pending, receipt, &leaf.path)?;
        let record = PublicationRecord {
            receipt: receipt.clone(),
            validation: validation.clone(),
            receipts,
        };
        records::write(
            &self.workspace.root.join("receipts").join(format!("{}.json", receipt.version.get())),
            &record,
        )?;
        self.workspace.config.warm_tip = node.id;
        records::write(&self.workspace.root.join("workspace.json"), &self.workspace.config)?;
        for (path, value) in &pending.changes {
            replace_origin(&mut leaf.origins, path, value.as_ref());
        }
        leaf.grants = self
            .authority
            .grants()?
            .into_iter()
            .filter(|grant| grant.leaf == leaf.id)
            .map(|grant| (grant.path.clone(), grant))
            .collect();
        leaf.sequence = std::num::NonZeroU64::new(
            pending
                .request
                .sequence
                .checked_add(1)
                .ok_or(cowtree_metadata::Error::CounterExhausted)?,
        )
        .ok_or(cowtree_metadata::Error::CounterExhausted)?;
        leaf.pending = None;
        leaf.last_receipt = Some(receipt.clone());
        self.save_leaf(&leaf)
    }

    fn batch_receipts(
        &self,
        pending: &crate::Pending,
        receipt: &Receipt,
        path: &std::path::Path,
    ) -> Result<Vec<Receipt>> {
        let mut receipts = Vec::new();
        if let Some(batch) = &pending.batch {
            for member in &batch.members {
                let result = self
                    .authority
                    .result(member.request)?
                    .ok_or_else(|| Issue::IncompleteRecord.at(path))?;
                if result.version != receipt.version || result.root != receipt.root {
                    return Err(Issue::InvalidCandidate.at(path));
                }
                receipts.push(result);
            }
        }
        Ok(receipts)
    }

    pub fn recover_publications(&mut self) -> Result<()> {
        for leaf in self.leaves()? {
            if let Some(pending) = &leaf.pending {
                if pending.submitted && !pending.aborting {
                    if let Some(receipt) = self.authority.result(pending.request)? {
                        self.finish_publication(leaf, &receipt)?;
                    }
                }
            }
        }
        Ok(())
    }
}
