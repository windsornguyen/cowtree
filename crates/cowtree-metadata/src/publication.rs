// Copyright (c) 2026 Windsor Nguyen

//! Capture, durable preparation, and one-transaction fenced publication.
//!
//! 1. Capture immutable values, origins, and lease generations under a request sequence.
//! 2. Pin the preparation parent, then pin its candidate before writing durable bytes.
//! 3. Mark only that attempt ready; superseded attempts cannot affect newer candidates.
//! 4. Recheck tip and authority, then commit epoch, origins, and retry receipt together.

use crate::database::{active, entry_json, parse_entry, read_snapshot, read_tip};
use crate::{
    Candidate, Error, Proposal, ProposalInput, ProposedChange, Receipt, RequestId, Result,
    Snapshot, Store, Token, Version, objects::ObjectId,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

#[derive(Clone, Copy, Eq, PartialEq)]
enum ProposalState {
    Pending,
    Committed,
    Aborted,
}
impl ProposalState {
    fn decode(value: String) -> rusqlite::Result<Self> {
        match value.as_str() {
            "pending" => Ok(Self::Pending),
            "committed" => Ok(Self::Committed),
            "aborted" => Ok(Self::Aborted),
            _ => Err(rusqlite::Error::InvalidQuery),
        }
    }
}

pub(crate) struct StoredProposal {
    pub(crate) input_hash: String,
    body: Option<String>,
    state: ProposalState,
    pub(crate) attempt: i64,
    pub(crate) parent: Option<i64>,
    root: Option<String>,
    pub(crate) ready: bool,
    committed_version: Option<i64>,
    pub(crate) batch_id: Option<String>,
}
impl StoredProposal {
    pub(crate) fn body(&self) -> Result<Proposal> {
        Ok(serde_json::from_str(self.body.as_deref().ok_or(Error::Aborted)?)?)
    }
    pub(crate) fn pending(&self) -> Result<()> {
        match self.state {
            ProposalState::Pending => Ok(()),
            ProposalState::Aborted => Err(Error::Aborted),
            ProposalState::Committed => Err(Error::RequestConflict),
        }
    }
    pub(crate) fn matches(&self, candidate: &Candidate) -> Result<()> {
        let attempt = i64::try_from(candidate.attempt).map_err(|_| Error::CandidateMismatch)?;
        if self.attempt != attempt
            || self.parent != Some(candidate.parent.sql())
            || self.root.as_deref() != Some(candidate.root.as_str())
        {
            return Err(Error::CandidateMismatch);
        }
        Ok(())
    }
    pub(crate) fn receipt(&self, request: RequestId) -> Result<Option<Receipt>> {
        if self.state != ProposalState::Committed {
            return Ok(None);
        }
        Ok(Some(Receipt {
            request,
            version: Version::from_sql(self.committed_version.ok_or(Error::Schema)?)?,
            root: ObjectId::parse(self.root.as_deref().ok_or(Error::Schema)?)?,
        }))
    }
}
enum CommitCheck {
    Committed(Receipt),
    Prepared(Snapshot),
}

impl Store {
    /// Freeze selected dirty values, origins, and tokens under one request identity.
    pub fn propose(&mut self, input: ProposalInput) -> Result<Proposal> {
        self.capacity()?;
        let sequence = input.request.seq()?;
        let input_hash = ObjectId::from_bytes(&serde_json::to_vec(&input.paths)?);
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(stored) = lookup(&tx, input.request)? {
            if stored.input_hash != input_hash.as_str() {
                return Err(Error::RequestConflict);
            }
            return stored.body();
        }
        let expected = active(&tx, input.request.leaf)?;
        if sequence < expected {
            return Err(Error::RequestExpired(sequence));
        }
        if sequence != expected {
            return Err(Error::RequestSequence { expected, actual: sequence });
        }
        if input.paths.is_empty() {
            return Err(Error::EmptyProposal);
        }
        let count: i64 =
            tx.query_row("SELECT count(*) FROM proposals WHERE state='pending'", [], |r| r.get(0))?;
        if count >= i64::from(self.limits.max_pending) {
            return Err(Error::Limit(crate::LimitKind::PendingProposals));
        }
        admit_proposal(&tx, &self.limits)?;
        let (version, _) = read_tip(&tx)?;
        let changes = input
            .paths
            .iter()
            .map(|path| capture(&tx, input.request, path))
            .collect::<Result<Vec<_>>>()?;
        let proposal =
            Proposal { request: input.request, captured_at: Version::from_sql(version)?, changes };
        let next = sequence.checked_add(1).ok_or(Error::CounterExhausted)?;
        tx.execute(
            "INSERT INTO proposals(leaf,sequence,input_hash,body,state,captured_at)
             VALUES(?1,?2,?3,?4,'pending',?5)",
            params![
                input.request.leaf.sql(),
                sequence,
                input_hash.as_str(),
                serde_json::to_string(&proposal)?,
                version
            ],
        )?;
        tx.execute(
            "UPDATE leaves SET next_sequence=?1 WHERE id=?2",
            params![next, input.request.leaf.sql()],
        )?;
        tx.commit()?;
        Ok(proposal)
    }
    /// Build a durable snapshot outside the metadata transaction, protected by pins.
    pub fn prepare(&mut self, request: RequestId) -> Result<Candidate> {
        self.capacity()?;
        let (proposal, parent, mut snapshot, attempt) = {
            let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let stored = required(&tx, request)?;
            stored.pending()?;
            let proposal = stored.body()?;
            let (parent, root) = read_tip(&tx)?;
            let snapshot = read_snapshot(&self.objects, &root)?;
            validate(&tx, &proposal, &snapshot)?;
            let attempt = stored.attempt.checked_add(1).ok_or(Error::CounterExhausted)?;
            tx.execute("UPDATE proposals SET attempt=?1,parent=?2,root=NULL,ready=0,batch_id=NULL WHERE leaf=?3 AND sequence=?4",
                params![attempt,parent,request.leaf.sql(),request.seq()?])?;
            tx.commit()?;
            (proposal, parent, snapshot, attempt)
        };
        for change in &proposal.changes {
            match &change.value {
                Some(entry) => {
                    snapshot.insert(change.path.clone(), entry.clone());
                }
                None => {
                    snapshot.remove(&change.path);
                }
            }
        }
        validate_namespace(&snapshot, self.limits.max_paths)?;
        let bytes = serde_json::to_vec(&snapshot)?;
        let root = ObjectId::from_bytes(&bytes);
        let candidate = Candidate {
            request,
            attempt: attempt as u64,
            parent: Version::from_sql(parent)?,
            root,
        };
        self.pin_candidate(&candidate)?;
        self.objects.put(&bytes)?;
        crate::fault::checkpoint("after-candidate-persist");
        self.ready_candidate(&candidate)?;
        Ok(candidate)
    }
    fn pin_candidate(&mut self, candidate: &Candidate) -> Result<()> {
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored = required(&tx, candidate.request)?;
        stored.pending()?;
        if stored.attempt as u64 != candidate.attempt {
            return Err(Error::CandidateMismatch);
        }
        tx.execute(
            "UPDATE proposals SET root=?1 WHERE leaf=?2 AND sequence=?3",
            params![
                candidate.root.as_str(),
                candidate.request.leaf.sql(),
                candidate.request.seq()?
            ],
        )?;
        tx.commit()?;
        Ok(())
    }
    fn ready_candidate(&mut self, candidate: &Candidate) -> Result<()> {
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored = required(&tx, candidate.request)?;
        stored.pending()?;
        stored.matches(candidate)?;
        tx.execute(
            "UPDATE proposals SET ready=1 WHERE leaf=?1 AND sequence=?2",
            params![candidate.request.leaf.sql(), candidate.request.seq()?],
        )?;
        tx.commit()?;
        Ok(())
    }
    fn check_commit(&mut self, candidate: &Candidate) -> Result<CommitCheck> {
        let root = {
            let tx = self.connection.transaction()?;
            let stored = single_candidate(&tx, candidate)?;
            if let Some(receipt) = stored.receipt(candidate.request)? {
                return Ok(CommitCheck::Committed(receipt));
            }
            let (tip, root) = read_tip(&tx)?;
            if tip != candidate.parent.sql() {
                return Err(Error::TipChanged { expected: candidate.parent.sql(), actual: tip });
            }
            tx.commit()?;
            root
        };
        crate::fault::checkpoint("before-commit-verification");
        let snapshot = verify_contents(&self.objects, &root, &candidate.root)?;
        crate::fault::checkpoint("after-commit-verification");
        Ok(CommitCheck::Prepared(snapshot))
    }

    /// Publish only the exact prepared candidate after rechecking tip, origins, and tokens.
    pub fn commit(&mut self, candidate: Candidate) -> Result<Receipt> {
        self.capacity()?;
        let snapshot = match self.check_commit(&candidate)? {
            CommitCheck::Committed(receipt) => return Ok(receipt),
            CommitCheck::Prepared(snapshot) => snapshot,
        };
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored = single_candidate(&tx, &candidate)?;
        if let Some(receipt) = stored.receipt(candidate.request)? {
            return Ok(receipt);
        }
        let (tip, _) = read_tip(&tx)?;
        if tip != candidate.parent.sql() {
            return Err(Error::TipChanged { expected: candidate.parent.sql(), actual: tip });
        }
        let proposal = stored.body()?;
        validate(&tx, &proposal, &snapshot)?;
        let version = tip.checked_add(1).ok_or(Error::CounterExhausted)?;
        tx.execute("INSERT INTO epochs VALUES(?1,?2)", params![version, candidate.root.as_str()])?;
        tx.execute("UPDATE settings SET tip=?1 WHERE singleton=1", [version])?;
        for change in &proposal.changes {
            let origin = entry_json(&change.value)?;
            tx.execute(
                "UPDATE leases SET origin=?1 WHERE path=?2",
                params![origin, change.path.as_str()],
            )?;
            tx.execute(
                "UPDATE views SET origin=?1 WHERE leaf=?2 AND path=?3",
                params![origin, candidate.request.leaf.sql(), change.path.as_str()],
            )?;
        }
        tx.execute("UPDATE proposals SET state='committed',committed_version=?1 WHERE leaf=?2 AND sequence=?3",
            params![version,candidate.request.leaf.sql(),candidate.request.seq()?])?;
        crate::fault::checkpoint("before-sql-commit");
        tx.commit()?;
        crate::fault::checkpoint("after-sql-commit");
        Ok(Receipt {
            request: candidate.request,
            version: Version::from_sql(version)?,
            root: candidate.root,
        })
    }
    /// Recover the acknowledgement for a committed request while its receipt is retained.
    pub fn result(&self, request: RequestId) -> Result<Option<Receipt>> {
        required(&self.connection, request)?.receipt(request)
    }
    /// Retire a pending request or the next unsubmitted identity without permitting reuse.
    pub fn abort(&mut self, request: RequestId) -> Result<()> {
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(stored) = lookup(&tx, request)? else {
            let sequence = request.seq()?;
            let expected = active(&tx, request.leaf)?;
            if sequence < expected {
                return Err(Error::RequestExpired(sequence));
            }
            if sequence != expected {
                return Err(Error::RequestSequence { expected, actual: sequence });
            }
            admit_proposal(&tx, &self.limits)?;
            let (version, _) = read_tip(&tx)?;
            tx.execute("INSERT INTO proposals(leaf,sequence,input_hash,state,captured_at) VALUES(?1,?2,'','aborted',?3)", params![request.leaf.sql(),sequence,version])?;
            let next = sequence.checked_add(1).ok_or(Error::CounterExhausted)?;
            tx.execute(
                "UPDATE leaves SET next_sequence=?1 WHERE id=?2",
                params![next, request.leaf.sql()],
            )?;
            tx.commit()?;
            return Ok(());
        };
        if stored.state == ProposalState::Aborted {
            return Ok(());
        }
        stored.pending()?;
        tx.execute(
            "UPDATE proposals SET state='aborted',body=NULL,root=NULL,parent=NULL,ready=0
             WHERE leaf=?1 AND sequence=?2",
            params![request.leaf.sql(), request.seq()?],
        )?;
        tx.commit()?;
        Ok(())
    }
}
pub(crate) fn lookup(
    connection: &Connection,
    request: RequestId,
) -> Result<Option<StoredProposal>> {
    let stored = connection
        .query_row(
            "SELECT input_hash,body,state,attempt,parent,root,ready,committed_version,batch_id
         FROM proposals WHERE leaf=?1 AND sequence=?2",
            params![request.leaf.sql(), request.seq()?],
            |r| {
                Ok(StoredProposal {
                    input_hash: r.get(0)?,
                    body: r.get(1)?,
                    state: ProposalState::decode(r.get(2)?)?,
                    attempt: r.get(3)?,
                    parent: r.get(4)?,
                    root: r.get(5)?,
                    ready: r.get(6)?,
                    committed_version: r.get(7)?,
                    batch_id: r.get(8)?,
                })
            },
        )
        .optional()?;
    Ok(stored)
}
pub(crate) fn required(connection: &Connection, request: RequestId) -> Result<StoredProposal> {
    lookup(connection, request)?.ok_or(Error::RequestExpired(request.seq()?))
}
fn capture(
    connection: &Connection,
    request: RequestId,
    path: &crate::ResourcePath,
) -> Result<ProposedChange> {
    let row: Option<(i64, bool, String, String, i64)> = connection.query_row(
        "SELECT token,activated,views.origin,value,revision FROM leases JOIN views USING(leaf,path)
         WHERE leaf=?1 AND path=?2",
        params![request.leaf.sql(),path.as_str()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
    let (token, activated, origin, value, revision) =
        row.ok_or_else(|| Error::StaleToken(path.as_str().into()))?;
    if !activated {
        return Err(Error::NotActivated(path.as_str().into()));
    }
    let origin = parse_entry(&origin)?;
    let value = parse_entry(&value)?;
    if origin == value {
        return Err(Error::EmptyProposal);
    }
    Ok(ProposedChange {
        path: path.clone(),
        token: Token::from_sql(token)?,
        origin,
        value,
        edit_revision: u64::try_from(revision).map_err(|_| Error::Schema)?,
    })
}
pub(crate) fn validate(
    connection: &Connection,
    proposal: &Proposal,
    snapshot: &Snapshot,
) -> Result<()> {
    active(connection, proposal.request.leaf)?;
    for change in &proposal.changes {
        let current: Option<(i64, i64, bool)> = connection
            .query_row(
                "SELECT leaf,token,activated FROM leases WHERE path=?1",
                [change.path.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        if current != Some((proposal.request.leaf.sql(), change.token.sql(), true)) {
            return Err(Error::StaleToken(change.path.as_str().into()));
        }
        if snapshot.get(&change.path) != change.origin.as_ref() {
            return Err(Error::StaleOrigin(change.path.as_str().into()));
        }
    }
    Ok(())
}
pub(crate) fn validate_namespace(snapshot: &Snapshot, max_paths: u32) -> Result<()> {
    if snapshot.len() > max_paths as usize {
        return Err(Error::Limit(crate::LimitKind::SnapshotPaths));
    }
    for (path, entry) in snapshot {
        if let Some(scope) = entry.kind.read_only_scope() {
            if !scope.contains(path) {
                return Err(Error::InvalidReadOnlyScope {
                    path: path.clone(),
                    scope: scope.clone(),
                });
            }
        }
        for (index, _) in path.as_str().match_indices('/') {
            let ancestor = crate::ResourcePath::parse(&path.as_str()[..index])?;
            if snapshot.contains_key(&ancestor) {
                return Err(Error::NamespaceConflict(path.as_str().into()));
            }
        }
    }
    Ok(())
}

pub(crate) fn admit_proposal(connection: &Connection, limits: &crate::Limits) -> Result<()> {
    let (epochs, pending, requests): (i64, i64, i64) = connection.query_row(
        "SELECT (SELECT count(*) FROM epochs),
         (SELECT count(*) FROM proposals WHERE state='pending'),
         (SELECT count(*) FROM proposals)",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let request_budget = 2 * i64::from(limits.retained_receipts) + i64::from(limits.max_pending);
    let epoch_budget = i64::from(limits.retained_epochs)
        + 4 * i64::from(limits.max_pending)
        + 2 * i64::from(limits.retained_receipts);
    if requests >= request_budget || epochs + pending >= epoch_budget {
        return Err(Error::Limit(crate::LimitKind::MetadataHistory));
    }
    Ok(())
}

fn single_candidate(connection: &Connection, candidate: &Candidate) -> Result<StoredProposal> {
    let stored = required(connection, candidate.request)?;
    if stored.batch_id.is_some() {
        return Err(Error::BatchConflict("candidate belongs to a batch".into()));
    }
    stored.matches(candidate)?;
    if stored.state != ProposalState::Committed {
        stored.pending()?;
        if !stored.ready {
            return Err(Error::CandidateNotReady);
        }
    }
    Ok(stored)
}

/// Pending parent and candidate pins retain all immutable bytes during verification.
/// A superseded or aborted pin can disappear; final metadata checks then forbid commit.
pub(crate) fn verify_contents(
    objects: &crate::objects::ObjectStore,
    parent: &str,
    candidate: &ObjectId,
) -> Result<Snapshot> {
    let snapshot = read_snapshot(objects, parent)?;
    let prepared = read_snapshot(objects, candidate.as_str())?;
    for entry in prepared.values() {
        objects.read(&entry.object)?;
    }
    Ok(snapshot)
}
