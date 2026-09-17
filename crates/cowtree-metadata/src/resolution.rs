// Copyright (c) 2026 Windsor Nguyen

//! Explicit same-owner conflict resolution without implicit winner selection.
//!
//! The caller names every superseded capture and selects one immutable value for
//! every affected path. Current activated grants authorize the replacement capture.
//! Sources retire atomically; local values and later edits remain untouched.

use crate::database::{active, parse_entry, read_snapshot, read_tip};
use crate::publication::{admit_proposal, lookup, required, validate_namespace};
use crate::{
    Error, Proposal, ProposedChange, RequestId, ResolutionInput, ResourcePath, Result, Store,
    Token, Version, objects::ObjectId,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::collections::BTreeMap;

impl Store {
    /// Replace explicitly selected same-owner captures with one newly fenced capture.
    pub fn resolve(&mut self, mut input: ResolutionInput) -> Result<Proposal> {
        self.capacity()?;
        validate_input(&mut input, self.limits.max_pending)?;
        let input_hash = ObjectId::from_bytes(&serde_json::to_vec(&input)?);
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(stored) = lookup(&tx, input.request)? {
            if stored.input_hash != input_hash.as_str() {
                return Err(Error::RequestConflict);
            }
            return stored.body();
        }
        let expected = active(&tx, input.request.leaf)?;
        let sequence = input.request.seq()?;
        if sequence < expected {
            return Err(Error::RequestExpired(sequence));
        }
        if sequence != expected {
            return Err(Error::RequestSequence { expected, actual: sequence });
        }
        admit_proposal(&tx, &self.limits)?;
        let sources = source_changes(&tx, &input)?;
        if sources.keys().ne(input.choices.keys()) {
            return Err(Error::ResolutionConflict("choices must cover every source path".into()));
        }
        let (tip, root) = read_tip(&tx)?;
        let mut snapshot = read_snapshot(&self.objects, &root)?;
        let mut changes = Vec::with_capacity(input.choices.len());
        for (path, chosen) in &input.choices {
            let change = selected_change(&tx, &input, path, *chosen, &sources)?;
            if snapshot.get(path) != change.origin.as_ref() {
                return Err(Error::StaleOrigin(path.as_str().into()));
            }
            match &change.value {
                Some(entry) => {
                    snapshot.insert(path.clone(), entry.clone());
                }
                None => {
                    snapshot.remove(path);
                }
            }
            changes.push(change);
        }
        if changes.iter().all(|change| change.origin == change.value) {
            return Err(Error::EmptyProposal);
        }
        validate_namespace(&snapshot, self.limits.max_paths)?;
        let proposal =
            Proposal { request: input.request, captured_at: Version::from_sql(tip)?, changes };
        insert_resolution(&tx, &input, &proposal, &input_hash)?;
        tx.commit()?;
        Ok(proposal)
    }
}

fn validate_input(input: &mut ResolutionInput, limit: u32) -> Result<()> {
    if input.sources.is_empty() || input.sources.len() > limit as usize {
        return Err(Error::ResolutionConflict("empty or oversized source set".into()));
    }
    input.sources.sort_by_key(|request| (request.leaf, request.sequence));
    if input.sources.windows(2).any(|pair| pair[0] == pair[1])
        || input
            .sources
            .iter()
            .any(|request| request.leaf != input.request.leaf || *request == input.request)
    {
        return Err(Error::ResolutionConflict(
            "sources must be distinct captures of this owner".into(),
        ));
    }
    Ok(())
}

type SourceChanges = BTreeMap<ResourcePath, Vec<(RequestId, ProposedChange)>>;

fn source_changes(connection: &Connection, input: &ResolutionInput) -> Result<SourceChanges> {
    let mut changes: SourceChanges = BTreeMap::new();
    for request in &input.sources {
        let stored = required(connection, *request)?;
        stored.pending()?;
        for change in stored.body()?.changes {
            changes.entry(change.path.clone()).or_default().push((*request, change));
        }
    }
    Ok(changes)
}

fn selected_change(
    connection: &Connection,
    input: &ResolutionInput,
    path: &ResourcePath,
    chosen: RequestId,
    sources: &SourceChanges,
) -> Result<ProposedChange> {
    let source = sources
        .get(path)
        .and_then(|changes| changes.iter().find(|(request, _)| *request == chosen))
        .ok_or_else(|| {
            Error::ResolutionConflict(format!(
                "selected capture does not contain {}",
                path.as_str()
            ))
        })?;
    let row: Option<(i64, bool, String, i64)> = connection
        .query_row(
            "SELECT token,activated,views.origin,revision FROM leases JOIN views USING(leaf,path)
         WHERE leaf=?1 AND path=?2",
            params![input.request.leaf.sql(), path.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let (token, activated, origin, revision) =
        row.ok_or_else(|| Error::StaleToken(path.as_str().into()))?;
    if !activated {
        return Err(Error::NotActivated(path.as_str().into()));
    }
    Ok(ProposedChange {
        path: path.clone(),
        token: Token::from_sql(token)?,
        origin: parse_entry(&origin)?,
        value: source.1.value.clone(),
        edit_revision: u64::try_from(revision).map_err(|_| Error::Schema)?,
    })
}

fn insert_resolution(
    connection: &Connection,
    input: &ResolutionInput,
    proposal: &Proposal,
    identity: &ObjectId,
) -> Result<()> {
    let sequence = input.request.seq()?;
    let next = sequence.checked_add(1).ok_or(Error::CounterExhausted)?;
    connection.execute(
        "INSERT INTO proposals(leaf,sequence,input_hash,body,state,captured_at)
        VALUES(?1,?2,?3,?4,'pending',?5)",
        params![
            input.request.leaf.sql(),
            sequence,
            identity.as_str(),
            serde_json::to_string(proposal)?,
            proposal.captured_at.sql()
        ],
    )?;
    connection.execute(
        "UPDATE leaves SET next_sequence=?1 WHERE id=?2",
        params![next, input.request.leaf.sql()],
    )?;
    for source in &input.sources {
        connection.execute(
            "UPDATE proposals SET state='aborted',body=NULL,root=NULL,parent=NULL,ready=0
            WHERE leaf=?1 AND sequence=?2",
            params![source.leaf.sql(), source.seq()?],
        )?;
    }
    Ok(())
}
