// Copyright (c) 2026 Windsor Nguyen

//! Atomic publication of explicitly selected, disjoint captured proposals.
//!
//! Every proposal carries the complete membership digest, its preparation generation,
//! and the same pinned snapshot. Partial, superseded, or regrouped candidates fail.

use crate::database::{entry_json, read_snapshot, read_tip};
use crate::publication::{required, validate, validate_namespace, verify_contents};
use crate::{
    BatchCandidate, BatchReceipt, Candidate, Error, Proposal, Receipt, RequestId, ResourcePath,
    Result, Snapshot, Store, Version, objects::ObjectId,
};
use rusqlite::{Connection, TransactionBehavior, params};
use std::collections::BTreeSet;

impl Store {
    /// Prepare one snapshot for a bounded set of disjoint pending requests.
    pub fn prepare_batch(
        &mut self,
        mut requests: Vec<RequestId>,
        expected_tip: Version,
    ) -> Result<BatchCandidate> {
        self.capacity()?;
        canonical_requests(&mut requests, self.limits.max_pending)?;
        let identity = ObjectId::from_bytes(&serde_json::to_vec(&requests)?);
        let (attempts, snapshot, proposals) = {
            let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let (tip, root) = read_tip(&tx)?;
            if tip != expected_tip.sql() {
                return Err(Error::TipChanged { expected: expected_tip.sql(), actual: tip });
            }
            let snapshot = read_snapshot(&self.objects, &root)?;
            let proposals = batch_proposals(&tx, &requests, &snapshot)?;
            let mut members = Vec::with_capacity(requests.len());
            for request in requests {
                let attempt = required(&tx, request)?
                    .attempt
                    .checked_add(1)
                    .ok_or(Error::CounterExhausted)?;
                tx.execute(
                    "UPDATE proposals SET attempt=?1,parent=?2,root=NULL,ready=0,batch_id=?3
                    WHERE leaf=?4 AND sequence=?5",
                    params![attempt, tip, identity.as_str(), request.leaf.sql(), request.seq()?],
                )?;
                members.push((request, attempt as u64));
            }
            tx.commit()?;
            (members, snapshot, proposals)
        };
        let snapshot = compose(snapshot, &proposals, self.limits.max_paths)?;
        let bytes = serde_json::to_vec(&snapshot)?;
        let root = ObjectId::from_bytes(&bytes);
        let members = attempts
            .into_iter()
            .map(|(request, attempt)| Candidate {
                request,
                attempt,
                parent: expected_tip,
                root: root.clone(),
            })
            .collect();
        let candidate = BatchCandidate { members };
        self.update_batch(&candidate, &identity, false)?;
        self.objects.put(&bytes)?;
        crate::fault::checkpoint("after-batch-candidate-persist");
        self.update_batch(&candidate, &identity, true)?;
        Ok(candidate)
    }

    fn update_batch(
        &mut self,
        batch: &BatchCandidate,
        identity: &ObjectId,
        ready: bool,
    ) -> Result<()> {
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for member in &batch.members {
            let stored = required(&tx, member.request)?;
            stored.pending()?;
            if stored.batch_id.as_deref() != Some(identity.as_str())
                || stored.attempt as u64 != member.attempt
                || stored.parent != Some(member.parent.sql())
            {
                return Err(Error::CandidateMismatch);
            }
            if ready {
                stored.matches(member)?;
            }
            tx.execute(
                "UPDATE proposals SET root=?1,ready=?2 WHERE leaf=?3 AND sequence=?4",
                params![
                    member.root.as_str(),
                    ready,
                    member.request.leaf.sql(),
                    member.request.seq()?
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Commit all members and receipts at one linearization point, or publish none.
    pub fn commit_batch(&mut self, candidate: BatchCandidate) -> Result<BatchReceipt> {
        self.capacity()?;
        let identity = batch_identity(&candidate, self.limits.max_pending)?;
        let first = candidate.members.first().ok_or(Error::EmptyProposal)?;
        let root = {
            let tx = self.connection.transaction()?;
            if let Some(receipts) = batch_receipts(&tx, &candidate, &identity)? {
                return Ok(BatchReceipt { receipts });
            }
            let (tip, root) = read_tip(&tx)?;
            if tip != first.parent.sql() {
                return Err(Error::TipChanged { expected: first.parent.sql(), actual: tip });
            }
            tx.commit()?;
            root
        };
        crate::fault::checkpoint("before-batch-commit-verification");
        let snapshot = verify_contents(&self.objects, &root, &first.root)?;
        crate::fault::checkpoint("after-batch-commit-verification");
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(receipts) = batch_receipts(&tx, &candidate, &identity)? {
            return Ok(BatchReceipt { receipts });
        }
        let (tip, _) = read_tip(&tx)?;
        if tip != first.parent.sql() {
            return Err(Error::TipChanged { expected: first.parent.sql(), actual: tip });
        }
        let requests: Vec<_> = candidate.members.iter().map(|m| m.request).collect();
        let proposals = batch_proposals(&tx, &requests, &snapshot)?;
        let version = tip.checked_add(1).ok_or(Error::CounterExhausted)?;
        tx.execute("INSERT INTO epochs VALUES(?1,?2)", params![version, first.root.as_str()])?;
        tx.execute("UPDATE settings SET tip=?1 WHERE singleton=1", [version])?;
        let mut receipts = Vec::with_capacity(candidate.members.len());
        for (member, proposal) in candidate.members.iter().zip(proposals) {
            publish_member(&tx, &proposal, version)?;
            receipts.push(Receipt {
                request: member.request,
                version: Version::from_sql(version)?,
                root: member.root.clone(),
            });
        }
        crate::fault::checkpoint("before-batch-sql-commit");
        tx.commit()?;
        crate::fault::checkpoint("after-batch-sql-commit");
        Ok(BatchReceipt { receipts })
    }
}

fn canonical_requests(requests: &mut [RequestId], limit: u32) -> Result<()> {
    if requests.is_empty() || requests.len() > limit as usize {
        return Err(Error::BatchConflict("empty or oversized membership".into()));
    }
    requests.sort_by_key(|request| (request.leaf, request.sequence));
    if requests.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(Error::BatchConflict("duplicate request".into()));
    }
    Ok(())
}

fn batch_identity(batch: &BatchCandidate, limit: u32) -> Result<ObjectId> {
    let original: Vec<_> = batch.members.iter().map(|member| member.request).collect();
    let mut sorted = original.clone();
    canonical_requests(&mut sorted, limit)?;
    let first = batch.members.first().ok_or(Error::EmptyProposal)?;
    if original != sorted
        || batch
            .members
            .iter()
            .any(|member| member.parent != first.parent || member.root != first.root)
    {
        return Err(Error::CandidateMismatch);
    }
    Ok(ObjectId::from_bytes(&serde_json::to_vec(&sorted)?))
}

fn batch_receipts(
    connection: &Connection,
    batch: &BatchCandidate,
    identity: &ObjectId,
) -> Result<Option<Vec<Receipt>>> {
    let mut receipts = Vec::new();
    for member in &batch.members {
        let stored = required(connection, member.request)?;
        if stored.batch_id.as_deref() != Some(identity.as_str()) {
            return Err(Error::CandidateMismatch);
        }
        stored.matches(member)?;
        if let Some(receipt) = stored.receipt(member.request)? {
            receipts.push(receipt);
        } else {
            stored.pending()?;
            if !stored.ready {
                return Err(Error::CandidateNotReady);
            }
        }
    }
    if receipts.is_empty() {
        return Ok(None);
    }
    if receipts.len() != batch.members.len()
        || receipts.iter().any(|receipt| receipt.version != receipts[0].version)
    {
        return Err(Error::BatchConflict("partial or inconsistent receipts".into()));
    }
    Ok(Some(receipts))
}

fn batch_proposals(
    connection: &Connection,
    requests: &[RequestId],
    snapshot: &Snapshot,
) -> Result<Vec<Proposal>> {
    let mut all_paths: BTreeSet<String> = BTreeSet::new();
    let mut proposals = Vec::with_capacity(requests.len());
    for request in requests {
        let stored = required(connection, *request)?;
        stored.pending()?;
        let proposal = stored.body()?;
        validate(connection, &proposal, snapshot)?;
        for change in &proposal.changes {
            if overlaps_set(&all_paths, &change.path) {
                return Err(Error::BatchConflict(change.path.as_str().into()));
            }
        }
        all_paths.extend(proposal.changes.iter().map(|change| change.path.as_str().to_owned()));
        proposals.push(proposal);
    }
    Ok(proposals)
}

fn overlaps_set(paths: &BTreeSet<String>, path: &ResourcePath) -> bool {
    let value = path.as_str();
    if paths.contains(value)
        || value.match_indices('/').any(|(index, _)| paths.contains(&value[..index]))
    {
        return true;
    }
    let prefix = format!("{value}/");
    paths.range(prefix.clone()..).next().is_some_and(|next| next.starts_with(&prefix))
}

fn compose(mut snapshot: Snapshot, proposals: &[Proposal], limit: u32) -> Result<Snapshot> {
    for change in proposals.iter().flat_map(|proposal| &proposal.changes) {
        match &change.value {
            Some(entry) => {
                snapshot.insert(change.path.clone(), entry.clone());
            }
            None => {
                snapshot.remove(&change.path);
            }
        }
    }
    validate_namespace(&snapshot, limit)?;
    Ok(snapshot)
}

fn publish_member(connection: &Connection, proposal: &Proposal, version: i64) -> Result<()> {
    for change in &proposal.changes {
        let origin = entry_json(&change.value)?;
        connection.execute(
            "UPDATE leases SET origin=?1 WHERE path=?2",
            params![origin, change.path.as_str()],
        )?;
        connection.execute(
            "UPDATE views SET origin=?1 WHERE leaf=?2 AND path=?3",
            params![origin, proposal.request.leaf.sql(), change.path.as_str()],
        )?;
    }
    connection.execute(
        "UPDATE proposals SET state='committed',committed_version=?1
        WHERE leaf=?2 AND sequence=?3",
        params![version, proposal.request.leaf.sql(), proposal.request.seq()?],
    )?;
    Ok(())
}
