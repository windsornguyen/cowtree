// Copyright (c) 2026 Windsor Nguyen

//! Transactional path reservations, logical activation, and retained leaf edits.

use std::collections::BTreeSet;

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::database::{active, entry_json, next_token, parse_entry, read_snapshot, read_tip};
use crate::objects::ObjectStore;
use crate::{Entry, EntryKind, Error, Grant, LeafId, LeafView, ResourcePath, Result, Store, Token};

impl Store {
    /// Reserve every requested path, or leave all reservations unchanged.
    pub fn acquire(&mut self, leaf: LeafId, paths: &BTreeSet<ResourcePath>) -> Result<Vec<Grant>> {
        self.capacity()?;
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        active(&tx, leaf)?;
        let (_, root) = read_tip(&tx)?;
        let snapshot = read_snapshot(&self.objects, &root)?;
        let installed = crate::installation::bound_snapshot(&tx, &self.objects, leaf)?;
        let mut grants = Vec::with_capacity(paths.len());
        for path in paths {
            exclude_other_holders(&tx, leaf, path)?;
            if let Some(grant) = read_grant(&tx, path)? {
                grants.push(grant);
                continue;
            }
            if installed.as_ref().is_some_and(|tree| tree.get(path) != snapshot.get(path)) {
                return Err(Error::TreeOutdated(leaf.sql()));
            }
            let view = read_view(&tx, leaf, path)?;
            if view.as_ref().is_some_and(LeafView::dirty) {
                return Err(Error::DirtyPath(path.as_str().to_owned()));
            }
            reserve_capacity(&tx, view.is_some(), self.limits.max_paths)?;
            let origin = snapshot.get(path).cloned();
            let token = Token::from_sql(next_token(&tx)?)?;
            tx.execute(
                "INSERT INTO leases VALUES(?1,?2,?3,?4,0)",
                params![path.as_str(), leaf.sql(), token.sql(), entry_json(&origin)?],
            )?;
            grants.push(Grant { leaf, path: path.clone(), token, origin, activated: false });
        }
        tx.commit()?;
        Ok(grants)
    }

    /// Install the captured origin in the logical view once, without touching a filesystem.
    pub fn activate(&mut self, grant: &Grant) -> Result<Grant> {
        self.capacity()?;
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut stored = validate_grant(&tx, grant)?;
        if !stored.activated {
            let old = read_view(&tx, grant.leaf, &grant.path)?;
            if old.as_ref().is_some_and(LeafView::dirty) {
                return Err(Error::DirtyPath(grant.path.as_str().to_owned()));
            }
            let revision = match old {
                Some(view) => next_revision(view.edit_revision)?,
                None => 0,
            };
            tx.execute(
                "INSERT INTO views(leaf,path,origin,value,revision,captured) VALUES(?1,?2,?3,?3,?4,0) ON CONFLICT(leaf,path) DO UPDATE
                 SET origin=excluded.origin,value=excluded.value,revision=excluded.revision,captured=0",
                params![
                    grant.leaf.sql(),
                    grant.path.as_str(),
                    entry_json(&stored.origin)?,
                    revision
                ],
            )?;
            tx.execute("UPDATE leases SET activated=1 WHERE path=?1", [grant.path.as_str()])?;
            stored.activated = true;
        }
        tx.commit()?;
        Ok(stored)
    }

    /// Stage a logical value under a live, activated grant and consume its upload pin.
    pub fn edit(&mut self, grant: &Grant, value: Option<Entry>) -> Result<LeafView> {
        self.edit_from(grant, value, false)
    }

    /// Record a value verified against the bound filesystem in the same edit transaction.
    pub(crate) fn edit_captured(
        &mut self,
        grant: &Grant,
        value: Option<Entry>,
    ) -> Result<LeafView> {
        self.edit_from(grant, value, true)
    }

    fn edit_from(
        &mut self,
        grant: &Grant,
        value: Option<Entry>,
        captured: bool,
    ) -> Result<LeafView> {
        self.capacity()?;
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored = validate_grant(&tx, grant)?;
        if !stored.activated {
            return Err(Error::NotActivated(grant.path.as_str().to_owned()));
        }
        let mut view = read_view(&tx, grant.leaf, &grant.path)?.ok_or(Error::Schema)?;
        if let Some(entry) = &value {
            validate_edit_object(&tx, &self.objects, grant, &view, entry)?;
        }
        let revision = next_revision(view.edit_revision)?;
        tx.execute(
            "UPDATE views SET value=?3,revision=?4,captured=?5 WHERE leaf=?1 AND path=?2",
            params![grant.leaf.sql(), grant.path.as_str(), entry_json(&value)?, revision, captured],
        )?;
        if let Some(entry) = &value {
            tx.execute(
                "DELETE FROM uploads WHERE leaf=?1 AND object=?2",
                params![grant.leaf.sql(), entry.object.as_str()],
            )?;
        }
        view.value = value;
        view.edit_revision = revision as u64;
        tx.commit()?;
        Ok(view)
    }

    /// Read all retained logical views from one consistent metadata snapshot.
    pub fn view(&self, leaf: LeafId) -> Result<Vec<LeafView>> {
        let tx = self.connection.unchecked_transaction()?;
        active(&tx, leaf)?;
        let views = {
            let mut statement = tx.prepare(
                "SELECT path,origin,value,revision FROM views WHERE leaf=?1 ORDER BY path",
            )?;
            let rows = statement.query_map([leaf.sql()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })?;
            let mut views = Vec::new();
            for row in rows {
                let (path, origin, value, revision) = row?;
                views.push(decode_view(ResourcePath::parse(path)?, origin, value, revision)?);
            }
            views
        };
        tx.commit()?;
        Ok(views)
    }

    /// Release authority while retaining the leaf's clean or dirty logical view.
    pub fn release(&mut self, grant: &Grant) -> Result<()> {
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        validate_grant(&tx, grant)?;
        tx.execute("DELETE FROM leases WHERE path=?1", [grant.path.as_str()])?;
        tx.commit()?;
        Ok(())
    }

    /// Administratively revoke an exact path; retained edits remain visible to their owner.
    pub fn revoke(&mut self, path: &ResourcePath) -> Result<()> {
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM leases WHERE path=?1", [path.as_str()])?;
        tx.commit()?;
        Ok(())
    }

    /// Discard local edits without rewriting any immutable captured proposal.
    pub fn discard(&mut self, leaf: LeafId, path: &ResourcePath) -> Result<()> {
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        active(&tx, leaf)?;
        let own_lease = read_grant(&tx, path)?.is_some_and(|grant| grant.leaf == leaf);
        if own_lease {
            if let Some(view) = read_view(&tx, leaf, path)? {
                tx.execute(
                    "UPDATE views SET value=origin,revision=?3,captured=0 WHERE leaf=?1 AND path=?2",
                    params![leaf.sql(), path.as_str(), next_revision(view.edit_revision)?],
                )?;
            }
        } else {
            tx.execute(
                "DELETE FROM views WHERE leaf=?1 AND path=?2",
                params![leaf.sql(), path.as_str()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Drop a leaf and its construction pins while preserving committed request receipts.
    pub fn drop_leaf(&mut self, leaf: LeafId) -> Result<()> {
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        active(&tx, leaf)?;
        tx.execute("DELETE FROM leases WHERE leaf=?1", [leaf.sql()])?;
        tx.execute("DELETE FROM views WHERE leaf=?1", [leaf.sql()])?;
        tx.execute("DELETE FROM uploads WHERE leaf=?1", [leaf.sql()])?;
        tx.execute(
            "UPDATE proposals SET state='aborted',body=NULL,parent=NULL,root=NULL,ready=0
             WHERE leaf=?1 AND state='pending'",
            [leaf.sql()],
        )?;
        tx.execute("DELETE FROM bindings WHERE leaf=?1", [leaf.sql()])?;
        tx.execute("DELETE FROM leaves WHERE id=?1", [leaf.sql()])?;
        tx.commit()?;
        Ok(())
    }
}

/// Validate the stored grant identity; the caller's activation flag is only a prior observation.
pub(crate) fn validate_grant(connection: &Connection, grant: &Grant) -> Result<Grant> {
    active(connection, grant.leaf)?;
    let stored = read_grant(connection, &grant.path)?
        .ok_or_else(|| Error::StaleToken(grant.path.as_str().to_owned()))?;
    if stored.leaf != grant.leaf || stored.token != grant.token || stored.origin != grant.origin {
        return Err(Error::StaleToken(grant.path.as_str().to_owned()));
    }
    Ok(stored)
}

fn read_grant(connection: &Connection, path: &ResourcePath) -> Result<Option<Grant>> {
    let raw = connection
        .query_row(
            "SELECT leaf,token,origin,activated FROM leases WHERE path=?1",
            [path.as_str()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, bool>(3)?,
                ))
            },
        )
        .optional()?;
    raw.map(|(leaf, token, origin, activated)| {
        Ok(Grant {
            leaf: LeafId::from_sql(leaf)?,
            path: path.clone(),
            token: Token::from_sql(token)?,
            origin: parse_entry(&origin)?,
            activated,
        })
    })
    .transpose()
}

fn read_view(
    connection: &Connection,
    leaf: LeafId,
    path: &ResourcePath,
) -> Result<Option<LeafView>> {
    let raw = connection
        .query_row(
            "SELECT origin,value,revision FROM views WHERE leaf=?1 AND path=?2",
            params![leaf.sql(), path.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?)),
        )
        .optional()?;
    raw.map(|(origin, value, revision)| decode_view(path.clone(), origin, value, revision))
        .transpose()
}

fn decode_view(
    path: ResourcePath,
    origin: String,
    value: String,
    revision: i64,
) -> Result<LeafView> {
    let view = LeafView {
        path,
        origin: parse_entry(&origin)?,
        value: parse_entry(&value)?,
        edit_revision: u64::try_from(revision).map_err(|_| Error::Schema)?,
    };
    Ok(view)
}

fn exclude_other_holders(connection: &Connection, leaf: LeafId, path: &ResourcePath) -> Result<()> {
    let mut statement = connection.prepare("SELECT path FROM leases WHERE leaf<>?1")?;
    for raw in statement.query_map([leaf.sql()], |row| row.get::<_, String>(0))? {
        if path.overlaps(&ResourcePath::parse(raw?)?) {
            return Err(Error::LeaseConflict(path.as_str().to_owned()));
        }
    }
    Ok(())
}

fn reserve_capacity(tx: &Transaction<'_>, has_view: bool, limit: u32) -> Result<()> {
    if has_view {
        return Ok(());
    }
    let count: i64 = tx.query_row(
        "SELECT (SELECT count(*) FROM views)+(SELECT count(*) FROM leases l
         WHERE NOT EXISTS(SELECT 1 FROM views v WHERE v.leaf=l.leaf AND v.path=l.path))",
        [],
        |row| row.get(0),
    )?;
    if count >= i64::from(limit) {
        return Err(Error::Limit(crate::LimitKind::RetainedPaths));
    }
    Ok(())
}

fn next_revision(revision: u64) -> Result<i64> {
    let current = i64::try_from(revision).map_err(|_| Error::Schema)?;
    current.checked_add(1).ok_or(Error::CounterExhausted)
}

fn validate_edit_object(
    connection: &Connection,
    objects: &ObjectStore,
    grant: &Grant,
    view: &LeafView,
    entry: &Entry,
) -> Result<()> {
    let already_owned = view.value.as_ref().is_some_and(|value| value.object == entry.object)
        || view.origin.as_ref().is_some_and(|origin| origin.object == entry.object);
    if !already_owned {
        let ready = connection
            .query_row(
                "SELECT ready FROM uploads WHERE leaf=?1 AND object=?2",
                params![grant.leaf.sql(), entry.object.as_str()],
                |row| row.get::<_, bool>(0),
            )
            .optional()?;
        if ready != Some(true) {
            return Err(Error::UploadNotReady);
        }
    }
    let bytes = objects.read(&entry.object)?;
    if entry.kind == EntryKind::Symlink
        && (bytes.is_empty() || bytes.contains(&0) || std::str::from_utf8(&bytes).is_err())
    {
        return Err(Error::InvalidSymlink(grant.path.as_str().to_owned()));
    }
    Ok(())
}
