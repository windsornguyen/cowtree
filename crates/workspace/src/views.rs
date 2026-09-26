// Copyright (c) 2026 Windsor Nguyen

//! Install published origins while preserving private edits and captured requests.

use std::{
    collections::BTreeSet,
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

use cowtree::CaptureMode;
use cowtree_metadata::{Entry, Grant, LeafId, ResourcePath, Snapshot, Token, Version};
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use crate::{
    Error, Leaf, Result, Workspace,
    error::Issue,
    installation::{Change, Direction, InstallRecord, Installation},
    manifest, paths, records,
    session::Session,
};

#[derive(Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ViewKind {
    Acquire,
    Sync,
    Discard,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ViewRecord {
    /// Protocol used to reconcile reservations after file installation.
    pub kind: ViewKind,
    /// Durable leaf state before the operation.
    pub before: Leaf,
    /// Reservation generations already owned before acquisition.
    pub tokens: Vec<Token>,
    /// Exact paths selected by this operation.
    pub paths: Vec<ResourcePath>,
    /// Acquired reservations awaiting acknowledgement.
    #[serde(default)]
    pub grants: Vec<Grant>,
    /// Ready client state once preparation finishes.
    pub after: Option<Leaf>,
}

impl Workspace {
    pub fn acquire(&self, identity: LeafId, requested: &[ResourcePath]) -> Result<Leaf> {
        self.lock()?.acquire(identity, requested)
    }

    pub fn sync(&self, identity: LeafId) -> Result<Leaf> {
        let mut session = self.lock()?;
        let leaf = session.read_leaf(identity)?;
        let current = session.current(&leaf)?;
        let (_, tip) = session.published()?;
        let mut blocked: BTreeSet<_> = current
            .keys()
            .chain(leaf.origins.keys())
            .filter(|path| current.get(*path) != leaf.origins.get(*path))
            .cloned()
            .collect();
        if let Some(pending) = &leaf.pending {
            blocked.extend(pending.changes.keys().cloned());
        }
        let selected: BTreeSet<_> = current
            .keys()
            .chain(leaf.origins.keys())
            .chain(tip.keys())
            .filter(|path| !blocked.contains(*path))
            .cloned()
            .collect();
        paths::aliases(current.keys().chain(tip.keys()).map(ResourcePath::as_str))?;
        let mut updated = leaf.clone();
        let mut changes = Vec::new();
        for path in &selected {
            replace_origin(&mut updated.origins, path, tip.get(path));
            if current.get(path) != tip.get(path) {
                changes.push(Change {
                    path: path.clone(),
                    before: current.get(path).cloned(),
                    after: tip.get(path).cloned(),
                });
            }
        }
        let warm = session.nodes().read(&session.workspace.config.warm_tip)?;
        if warm.source != tip {
            return Err(Issue::NodeChanged.at(&self.root));
        }
        updated.git_head = warm.git_commit;
        let record = ViewRecord {
            kind: ViewKind::Sync,
            before: leaf,
            tokens: Vec::new(),
            paths: selected.into_iter().collect(),
            grants: Vec::new(),
            after: Some(updated.clone()),
        };
        Installation::preflight(&install_record(&updated.path, changes.clone()))?;
        let directory = session.begin_view(&record)?;
        session.prepare_installation(&directory, &updated.path, changes)?;
        session.complete_view(&directory, &record)?;
        session.pin_origins()?;
        Ok(updated)
    }

    pub fn discard(&self, identity: LeafId, requested: &[ResourcePath]) -> Result<Leaf> {
        let mut session = self.lock()?;
        let leaf = session.read_leaf(identity)?;
        let current = session.current(&leaf)?;
        let selected: BTreeSet<_> = requested
            .iter()
            .map(|path| ResourcePath::parse(path.as_str().nfc().collect::<String>()))
            .collect::<cowtree_metadata::Result<_>>()?;
        if selected.is_empty() {
            return Err(Issue::InvalidPath.at(&leaf.path));
        }
        let mut baseline = leaf.origins.clone();
        if let Some(pending) = &leaf.pending {
            for (path, value) in &pending.changes {
                replace_origin(&mut baseline, path, value.as_ref());
            }
        }
        for path in &selected {
            paths::local(&leaf.path, path.as_str())?;
        }
        let changes: Vec<_> = selected
            .iter()
            .filter(|path| current.get(*path) != baseline.get(*path))
            .map(|path| Change {
                path: path.clone(),
                before: current.get(path).cloned(),
                after: baseline.get(path).cloned(),
            })
            .collect();
        let record = ViewRecord {
            kind: ViewKind::Discard,
            before: leaf.clone(),
            tokens: Vec::new(),
            paths: selected.into_iter().collect(),
            grants: Vec::new(),
            after: Some(leaf.clone()),
        };
        Installation::preflight(&install_record(&leaf.path, changes.clone()))?;
        let directory = session.begin_view(&record)?;
        session.prepare_installation(&directory, &leaf.path, changes)?;
        session.complete_view(&directory, &record)?;
        session.pin_origins()?;
        Ok(leaf)
    }
}

impl Session {
    pub(crate) fn acquire(&mut self, identity: LeafId, requested: &[ResourcePath]) -> Result<Leaf> {
        let leaf = self.read_leaf(identity)?;
        let current = self.current(&leaf)?;
        let (_, tip) = self.published()?;
        let requested: BTreeSet<_> = requested
            .iter()
            .map(|path| ResourcePath::parse(path.as_str().nfc().collect::<String>()))
            .collect::<cowtree_metadata::Result<_>>()?;
        if requested.is_empty() {
            return Err(Issue::InvalidPath.at(&leaf.path));
        }
        for path in &requested {
            paths::local(&leaf.path, path.as_str())?;
        }
        let mut selected = requested.clone();
        selected.extend(
            current
                .keys()
                .chain(tip.keys())
                .filter(|path| requested.iter().any(|prefix| prefix.contains(path)))
                .cloned(),
        );
        let grants = self.authority.grants()?;
        paths::aliases(
            current
                .keys()
                .chain(tip.keys())
                .chain(&selected)
                .chain(grants.iter().map(|grant| &grant.path))
                .map(ResourcePath::as_str),
        )?;
        let mut record = ViewRecord {
            kind: ViewKind::Acquire,
            before: leaf.clone(),
            tokens: grants
                .iter()
                .filter(|grant| grant.leaf == identity)
                .map(|grant| grant.token)
                .collect(),
            paths: selected.iter().cloned().collect(),
            grants: Vec::new(),
            after: None,
        };
        let directory = self.begin_view(&record)?;
        record.grants = self.authority.acquire(identity, &selected)?;
        let (updated, changes) = acquisition(leaf, &current, &record.grants)?;
        record.after = Some(updated.clone());
        records::write(&directory.join("view.json"), &record)?;
        self.prepare_installation(&directory, &updated.path, changes)?;
        self.complete_view(&directory, &record)?;
        self.pin_origins()?;
        Ok(updated)
    }

    pub(crate) fn validate_leaf(&self, leaf: &Leaf) -> Result<()> {
        let identity =
            fs::symlink_metadata(&leaf.path).map_err(|error| Error::io(&leaf.path, error))?;
        if !identity.is_dir() || (identity.dev(), identity.ino()) != (leaf.device, leaf.inode) {
            return Err(Issue::ChangedDirectory.at(&leaf.path));
        }
        let repository = self.repository().select(&leaf.path);
        repository.detached(std::slice::from_ref(&leaf.git_head))?;
        Ok(())
    }

    pub fn current(&self, leaf: &Leaf) -> Result<Snapshot> {
        self.validate_leaf(leaf)?;
        let policy = self.workspace.config.policy.working(&leaf.path)?;
        let entries = cowtree::scan_tree(&leaf.path, &policy.compile()?, CaptureMode::Content)?;
        manifest::capture(&leaf.path, &entries, &policy)
    }

    pub fn published(&mut self) -> Result<(Version, Snapshot)> {
        let (version, _) = self.authority.tip()?;
        Ok((version, self.authority.snapshot(version)?))
    }

    pub fn begin_view(&self, record: &ViewRecord) -> Result<PathBuf> {
        let directory = self
            .workspace
            .root
            .join("operations")
            .join(format!("view-{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir(&directory).map_err(|error| Error::io(&directory, error))?;
        records::write(&directory.join("view.json"), record)?;
        Ok(directory)
    }

    pub fn prepare_installation(
        &self,
        directory: &Path,
        root: &Path,
        changes: Vec<Change>,
    ) -> Result<()> {
        Installation::prepare(
            &directory.join("installation"),
            install_record(root, changes),
            &self.authority.object_directory(),
        )?;
        Ok(())
    }

    pub fn complete_view(&mut self, directory: &Path, record: &ViewRecord) -> Result<()> {
        let after = record.after.as_ref().ok_or_else(|| Issue::IncompleteRecord.at(directory))?;
        if record.kind == ViewKind::Discard || self.read_leaf(record.before.id)? != *after {
            Installation::open(&directory.join("installation"))?.apply(Direction::Apply)?;
            crate::fault::checkpoint("after-view-installation");
            for grant in &record.grants {
                self.authority.activate(grant)?;
            }
            if after.git_head != record.before.git_head {
                let target = self.repository().select(&record.before.path);
                target.detached(&[record.before.git_head.clone(), after.git_head.clone()])?;
                target.reset_index(&after.git_head)?;
            }
            self.save_leaf(after)?;
        }
        records::remove_directory(directory)
    }

    fn abort_view(&mut self, directory: &Path, record: &ViewRecord) -> Result<()> {
        if directory.join("installation/install.json").exists() {
            Installation::open(&directory.join("installation"))?.apply(Direction::Rollback)?;
        }
        if record.kind == ViewKind::Acquire {
            for grant in self.authority.grants()? {
                if grant.leaf == record.before.id
                    && !record.tokens.contains(&grant.token)
                    && record.paths.contains(&grant.path)
                {
                    self.authority.release(&grant)?;
                }
            }
        }
        records::remove_directory(directory)
    }

    pub fn recover_views(&mut self) -> Result<()> {
        for directory in paths::entries(&self.workspace.root.join("operations"))? {
            if !directory
                .file_name()
                .is_some_and(|name| name.as_encoded_bytes().starts_with(b"view-"))
            {
                continue;
            }
            if !directory.join("view.json").exists() {
                records::remove_directory(&directory)?;
                continue;
            }
            let record: ViewRecord = records::read(&directory.join("view.json"))?;
            if record.after.is_none() || !directory.join("installation/install.json").exists() {
                self.abort_view(&directory, &record)?;
                continue;
            }
            match self.complete_view(&directory, &record) {
                Err(Error::Authority(
                    cowtree_metadata::Error::StaleToken(_)
                    | cowtree_metadata::Error::LeafInactive(_),
                )) => self.abort_view(&directory, &record)?,
                result => result?,
            }
        }
        Ok(())
    }
}

pub(crate) fn replace_origin(origins: &mut Snapshot, path: &ResourcePath, value: Option<&Entry>) {
    match value {
        Some(value) => {
            origins.insert(path.clone(), value.clone());
        }
        None => {
            origins.remove(path);
        }
    }
}

pub(crate) fn install_record(root: &Path, changes: Vec<Change>) -> InstallRecord {
    InstallRecord { root: root.into(), changes, device: 0, inode: 0 }
}

fn acquisition(
    mut leaf: Leaf,
    current: &Snapshot,
    grants: &[Grant],
) -> Result<(Leaf, Vec<Change>)> {
    let mut changes = Vec::new();
    for grant in grants {
        let value = current.get(&grant.path);
        let original = leaf.origins.get(&grant.path);
        let dirty = value != original;
        if dirty && original != grant.origin.as_ref() && value != grant.origin.as_ref() {
            return Err(Issue::SourceChanged.at(&leaf.path.join(grant.path.as_str())));
        }
        let desired =
            if dirty && original == grant.origin.as_ref() { value } else { grant.origin.as_ref() };
        if desired != value || grant.origin.as_ref() != original {
            changes.push(Change {
                path: grant.path.clone(),
                before: value.cloned(),
                after: desired.cloned(),
            });
        }
        replace_origin(&mut leaf.origins, &grant.path, grant.origin.as_ref());
        let mut held = grant.clone();
        held.activated = true;
        leaf.grants.insert(grant.path.clone(), held);
    }
    Ok((leaf, changes))
}
