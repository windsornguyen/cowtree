// Copyright (c) 2026 Windsor Nguyen

//! Bound worktrees: durable install intent, idempotent file replacement, verified acknowledgement.
//!
//! 1. Pin the target epoch and record a plan before replacing any path.
//! 2. Replace only expected old values; preserve explicitly captured later local edits.
//! 3. Flush and verify every resulting value before acknowledging the installed version.
//!
//! Recovery rolls the durable plan forward. Editors must remain quiescent during installation.

use crate::{EntryKind, Error, Grant, LeafId, LeafView, Result, Snapshot, Store, Version};
use crate::{
    database::{active, next_token},
    tree_io,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Installation {
    /// Durable identity of the bound logical leaf.
    pub leaf: LeafId,
    /// Canonical directory whose device/inode were checked when bound.
    pub path: PathBuf,
    /// Last fully verified and durably acknowledged installation.
    pub version: Version,
    /// Retained target that must be recovered before further leaf edits.
    pub pending: Option<Version>,
}
struct Bound {
    installation: Installation,
    device: i64,
    inode: i64,
    generation: i64,
    plan: Option<String>,
}

impl Store {
    pub(crate) fn filesystem_lock(&self) -> Result<File> {
        let path = self.root.join("workspace.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| tree_io::io(&path, error))?;
        rustix::fs::flock(&file, rustix::fs::FlockOperation::LockExclusive)
            .map_err(|error| tree_io::io(&path, error.into()))?;
        Ok(file)
    }
    pub fn binding(&self, leaf: LeafId) -> Result<Installation> {
        Ok(bound(&self.connection, leaf)?.installation)
    }
    pub fn bind_tree(
        &mut self,
        leaf: LeafId,
        path: &Path,
        version: Version,
    ) -> Result<Installation> {
        let _lock = self.filesystem_lock()?;
        let path = fs::canonicalize(path).map_err(|error| tree_io::io(path, error))?;
        let (device, inode) = tree_io::identity(&path)?;
        if device != tree_io::identity(&self.root)?.0 {
            return Err(Error::InstallConflict(
                "worktree and authority must share a filesystem".into(),
            ));
        }
        if path.starts_with(&self.root) || self.root.starts_with(&path) {
            return Err(Error::BindingChanged(path.display().to_string()));
        }
        tree_io::probe_clone(&self.root)?;
        let snapshot = self.snapshot(version)?;
        crate::tree_io::validate_names(&self.root, &snapshot)?;
        for (name, expected) in &snapshot {
            if tree_io::read(&path, name, self.limits.max_object_bytes)?.map(|v| v.0).as_ref()
                != Some(expected)
            {
                return Err(Error::TreeChanged(name.as_str().into()));
            }
        }
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        active(&tx, leaf)?;
        match bound(&tx, leaf) {
            Ok(previous) => {
                if previous.installation.path != path
                    || previous.installation.version != version
                    || previous.device != device
                    || previous.inode != inode
                {
                    return Err(Error::BindingChanged(path.display().to_string()));
                }
                return Ok(previous.installation);
            }
            Err(Error::UnboundLeaf(_)) => {}
            Err(error) => return Err(error),
        }
        let mut statement = tx.prepare("SELECT path,device,inode FROM bindings")?;
        let others = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?))
        })?;
        for other in others {
            let (other, other_device, other_inode) = other?;
            let other = Path::new(&other);
            if path.starts_with(other)
                || other.starts_with(&path)
                || (device, inode) == (other_device, other_inode)
            {
                return Err(Error::BindingChanged(path.display().to_string()));
            }
        }
        drop(statement);
        tx.execute(
            "INSERT INTO bindings(leaf,path,device,inode,version) VALUES(?1,?2,?3,?4,?5)",
            params![
                leaf.sql(),
                path.to_str().ok_or(Error::ImportConflict)?,
                device,
                inode,
                version.sql()
            ],
        )?;
        tx.commit()?;
        Ok(Installation { leaf, path, version, pending: None })
    }
    pub fn capture_files(&mut self, grants: &[Grant]) -> Result<Vec<LeafView>> {
        let _lock = self.filesystem_lock()?;
        let mut views = Vec::new();
        for grant in grants {
            let binding = bound(&self.connection, grant.leaf)?;
            verify_identity(&binding)?;
            crate::leases::validate_grant(&self.connection, grant)?;
            let value = tree_io::read(
                &binding.installation.path,
                &grant.path,
                self.limits.max_object_bytes,
            )?;
            if let Some((entry, bytes)) = &value {
                match entry.kind {
                    EntryKind::Symlink => {
                        self.stage(grant.leaf, bytes)?;
                    }
                    EntryKind::File | EntryKind::Executable => {
                        self.stage_file(
                            grant.leaf,
                            &binding.installation.path.join(grant.path.as_str()),
                            entry.object.clone(),
                        )?;
                    }
                }
            }
            views.push(self.edit_captured(grant, value.map(|v| v.0))?);
        }
        Ok(views)
    }
    pub fn install(&mut self, leaf: LeafId, version: Version) -> Result<Installation> {
        let _lock = self.filesystem_lock()?;
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        active(&tx, leaf)?;
        let binding = bound(&tx, leaf)?;
        verify_identity(&binding)?;
        if version < binding.installation.version {
            return Err(Error::InstallConflict(
                "cannot install an older acknowledged epoch".into(),
            ));
        }
        let old = snapshot_at(&tx, &self.objects, binding.installation.version)?;
        let new = snapshot_at(&tx, &self.objects, version)?;
        crate::tree_io::validate_names(&self.root, &new)?;
        let plan = crate::install_plan::build(
            &tx,
            &binding.installation,
            &old,
            &new,
            self.limits.max_object_bytes,
        )?;
        crate::install_plan::validate_effective(&self.root, &new, &plan, self.limits.max_paths)?;
        let generation = next_token(&tx)?;
        let stage = self.root.join(format!(".install-{}-{generation}", leaf.get()));
        match fs::symlink_metadata(&stage) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(tree_io::io(&stage, error)),
            Ok(_) => return Err(Error::PathConflict(stage.display().to_string())),
        }
        tx.execute(
            "UPDATE bindings SET pending_version=?1,generation=?2,plan=?3 WHERE leaf=?4",
            params![version.sql(), generation, serde_json::to_string(&plan)?, leaf.sql()],
        )?;
        tx.commit()?;
        crate::fault::checkpoint("after-install-intent");
        self.finish_install(leaf)
    }
    pub fn recover(&mut self, leaf: LeafId) -> Result<Installation> {
        let _lock = self.filesystem_lock()?;
        self.finish_install(leaf)
    }
    fn finish_install(&mut self, leaf: LeafId) -> Result<Installation> {
        let binding = bound(&self.connection, leaf)?;
        verify_identity(&binding)?;
        let Some(version) = binding.installation.pending else {
            return Ok(binding.installation);
        };
        let changes: Vec<crate::install_plan::Change> =
            serde_json::from_str(binding.plan.as_deref().ok_or(Error::Schema)?)?;
        let stage = self.root.join(format!(".install-{}-{}", leaf.get(), binding.generation));
        match fs::create_dir(&stage) {
            Ok(()) => tree_io::sync(&self.root)?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                tree_io::identity(&stage)?;
            }
            Err(error) => return Err(tree_io::io(&stage, error)),
        }
        for (index, change) in changes.iter().enumerate() {
            crate::install_plan::Files {
                objects: &self.objects,
                root: &binding.installation.path,
                stage: &stage,
                limit: self.limits.max_object_bytes,
            }
            .apply(index, change)?;
            crate::fault::checkpoint("after-install-path");
        }
        for change in &changes {
            if tree_io::read(
                &binding.installation.path,
                &change.path,
                self.limits.max_object_bytes,
            )?
            .map(|v| v.0)
                != change.after
            {
                return Err(Error::TreeChanged(change.path.as_str().into()));
            }
        }
        fs::remove_dir(&stage).map_err(|error| tree_io::io(&stage, error))?;
        tree_io::sync(&self.root)?;
        tree_io::sync(&binding.installation.path)?;
        crate::fault::checkpoint("before-install-ack");
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("UPDATE bindings SET version=?1,pending_version=NULL,plan=NULL WHERE leaf=?2 AND generation=?3",
            params![version.sql(),leaf.sql(),binding.generation])?;
        tx.commit()?;
        crate::fault::checkpoint("after-install-ack");
        self.binding(leaf)
    }
}
struct BoundRow {
    path: String,
    device: i64,
    inode: i64,
    version: i64,
    pending: Option<i64>,
    generation: i64,
    plan: Option<String>,
}
fn bound(connection: &Connection, leaf: LeafId) -> Result<Bound> {
    let row=connection.query_row(
        "SELECT path,device,inode,version,pending_version,generation,plan FROM bindings WHERE leaf=?1",[leaf.sql()],
        |r| Ok(BoundRow { path:r.get(0)?, device:r.get(1)?, inode:r.get(2)?, version:r.get(3)?, pending:r.get(4)?, generation:r.get(5)?, plan:r.get(6)? })).optional()?;
    let BoundRow { path, device, inode, version, pending, generation, plan } =
        row.ok_or(Error::UnboundLeaf(leaf.sql()))?;
    Ok(Bound {
        installation: Installation {
            leaf,
            path: PathBuf::from(path),
            version: Version::from_sql(version)?,
            pending: pending.map(Version::from_sql).transpose()?,
        },
        device,
        inode,
        generation,
        plan,
    })
}
fn verify_identity(binding: &Bound) -> Result<()> {
    if tree_io::identity(&binding.installation.path)? != (binding.device, binding.inode) {
        return Err(Error::BindingChanged(binding.installation.path.display().to_string()));
    }
    Ok(())
}
fn snapshot_at(
    connection: &Connection,
    objects: &crate::objects::ObjectStore,
    version: Version,
) -> Result<Snapshot> {
    let root: String = connection
        .query_row("SELECT root FROM epochs WHERE version=?1", [version.sql()], |r| r.get(0))
        .optional()?
        .ok_or(Error::SnapshotExpired(version.sql()))?;
    crate::database::read_snapshot(objects, &root)
}
pub(crate) fn bound_snapshot(
    connection: &Connection,
    objects: &crate::objects::ObjectStore,
    leaf: LeafId,
) -> Result<Option<Snapshot>> {
    let version: Option<i64> = connection
        .query_row("SELECT version FROM bindings WHERE leaf=?1", [leaf.sql()], |r| r.get(0))
        .optional()?;
    version.map(|version| snapshot_at(connection, objects, Version::from_sql(version)?)).transpose()
}
