// Copyright (c) 2026 Windsor Nguyen

//! Install caller-selected local or published values under fresh fenced grants.

use std::collections::BTreeMap;

use cowtree_metadata::{LeafId, ResourcePath};
use serde::{Deserialize, Serialize};

use crate::{
    Leaf, Result, Workspace,
    error::Issue,
    installation::Change,
    paths, records,
    views::{ViewKind, ViewRecord, replace_origin},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Choice {
    Local,
    Published,
}

impl Workspace {
    pub fn resolve(
        &self,
        identity: LeafId,
        choices: &BTreeMap<ResourcePath, Choice>,
    ) -> Result<Leaf> {
        if choices.is_empty() {
            return Err(Issue::InvalidPath.at(&self.root));
        }
        let mut session = self.lock()?;
        let leaf = session.read_leaf(identity)?;
        if leaf.pending.is_some() {
            return Err(Issue::PendingPublication.at(&leaf.path));
        }
        if leaf.check_candidate.is_some() {
            return Err(Issue::InvalidCandidate.at(&leaf.path));
        }
        let current = session.current(&leaf)?;
        for path in choices.keys() {
            paths::local(&leaf.path, path.as_str())?;
        }
        let live = session.authority.grants()?;
        paths::aliases(
            current
                .keys()
                .chain(choices.keys())
                .chain(live.iter().map(|grant| &grant.path))
                .map(ResourcePath::as_str),
        )?;
        let mut record = ViewRecord {
            kind: ViewKind::Acquire,
            before: leaf.clone(),
            paths: choices.keys().cloned().collect(),
            tokens: live
                .iter()
                .filter(|grant| grant.leaf == identity)
                .map(|grant| grant.token)
                .collect(),
            grants: Vec::new(),
            after: None,
        };
        let directory = session.begin_view(&record)?;
        let grants = session.authority.acquire(identity, &choices.keys().cloned().collect())?;
        let mut updated = leaf;
        let mut changes = Vec::new();
        for grant in &grants {
            let choice = choices
                .get(&grant.path)
                .ok_or_else(|| Issue::IncompleteRecord.at(&updated.path))?;
            let desired = match choice {
                Choice::Local => current.get(&grant.path),
                Choice::Published => grant.origin.as_ref(),
            };
            changes.push(Change {
                path: grant.path.clone(),
                before: current.get(&grant.path).cloned(),
                after: desired.cloned(),
            });
            replace_origin(&mut updated.origins, &grant.path, grant.origin.as_ref());
            let mut held = grant.clone();
            held.activated = true;
            updated.grants.insert(grant.path.clone(), held);
        }
        record.grants = grants;
        record.after = Some(updated.clone());
        records::write(&directory.join("view.json"), &record)?;
        session.prepare_installation(&directory, &updated.path, changes)?;
        session.complete_view(&directory, &record)?;
        session.pin_origins()?;
        Ok(updated)
    }
}
