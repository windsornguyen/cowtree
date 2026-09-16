// Copyright (c) 2026 Windsor Nguyen

//! Plan namespace changes and materialize one idempotent installation step.

use crate::database::parse_entry;
use crate::{Entry, EntryKind, Error, LeafId, ResourcePath, Result, Snapshot, tree_io};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    os::unix::fs::PermissionsExt,
    path::Path,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Change {
    pub(crate) path: ResourcePath,
    pub(crate) before: Option<Entry>,
    pub(crate) after: Option<Entry>,
}
pub(crate) fn build(
    connection: &Connection,
    binding: &crate::Installation,
    old: &Snapshot,
    new: &Snapshot,
    limit: u64,
) -> Result<Vec<Change>> {
    let paths: BTreeSet<_> = old.keys().chain(new.keys()).cloned().collect();
    let mut plan = Vec::new();
    for path in paths {
        if old.get(&path) == new.get(&path) {
            continue;
        }
        let current = planned_current(&binding.path, &path, old, new, limit)?;
        let target = new.get(&path).cloned();
        let intent = local_intent(connection, binding.leaf, &path)?;
        let owned = intent.as_ref().is_some_and(|intent| intent.origin == target);
        if owned && intent.as_ref().is_some_and(|intent| intent.captured && intent.value != current)
        {
            return Err(Error::DirtyPath(path.as_str().into()));
        }
        let retained = owned && intent.as_ref().is_some_and(|intent| intent.value == current);
        if !old.contains_key(&path) && current.is_some() && !retained {
            return Err(Error::PathConflict(path.as_str().into()));
        }
        if target.is_some() {
            validate_directory_replacement(&binding.path, &path, old, new)?;
        }
        let after = if retained {
            current.clone()
        } else if current == old.get(&path).cloned() || current == target {
            target
        } else {
            return Err(Error::DirtyPath(path.as_str().into()));
        };
        plan.push(Change { path, before: current, after });
    }
    plan.sort_by_key(|change| (change.after.is_some(), change.path.clone()));
    Ok(plan)
}
struct LocalIntent {
    origin: Option<Entry>,
    value: Option<Entry>,
    captured: bool,
}
fn local_intent(
    connection: &Connection,
    leaf: LeafId,
    path: &ResourcePath,
) -> Result<Option<LocalIntent>> {
    let row: Option<(String, String, bool)> = connection
        .query_row(
            "SELECT origin,value,captured FROM views WHERE leaf=?1 AND path=?2",
            params![leaf.sql(), path.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    row.map(|(origin, value, captured)| {
        Ok(LocalIntent { origin: parse_entry(&origin)?, value: parse_entry(&value)?, captured })
    })
    .transpose()
}
pub(crate) fn validate_effective(
    authority: &Path,
    new: &Snapshot,
    changes: &[Change],
    max_paths: u32,
) -> Result<()> {
    if changes.iter().all(|change| change.after.as_ref() == new.get(&change.path)) {
        return Ok(());
    }
    let mut effective = new.clone();
    for change in changes {
        match &change.after {
            Some(entry) => {
                effective.insert(change.path.clone(), entry.clone());
            }
            None => {
                effective.remove(&change.path);
            }
        }
    }
    crate::publication::validate_namespace(&effective, max_paths)?;
    tree_io::validate_names(authority, &effective)
}
pub(crate) struct Files<'a> {
    pub(crate) objects: &'a crate::objects::ObjectStore,
    pub(crate) root: &'a Path,
    pub(crate) stage: &'a Path,
    pub(crate) limit: u64,
}
impl Files<'_> {
    pub(crate) fn apply(&self, index: usize, change: &Change) -> Result<()> {
        let Self { root, stage, limit, .. } = *self;
        let current = tree_io::read(root, &change.path, limit)?.map(|v| v.0);
        let target = root.join(change.path.as_str());
        if current == change.after {
            let temporary = stage.join(index.to_string());
            match fs::remove_file(&temporary) {
                Ok(()) => tree_io::sync(stage)?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(tree_io::io(&temporary, error)),
            }
            if change.after.is_none() && tree_io::checked(root, &change.path).is_ok() {
                tree_io::remove_empty_parents(root, &target)?;
            }
            return Ok(());
        }
        tree_io::checked(root, &change.path)?;
        if current != change.before {
            return Err(Error::TreeChanged(change.path.as_str().into()));
        }
        let Some(entry) = &change.after else {
            fs::remove_file(&target).map_err(|error| tree_io::io(&target, error))?;
            if let Some(parent) = target.parent() {
                tree_io::sync(parent)?;
            }
            return tree_io::remove_empty_parents(root, &target);
        };
        if fs::symlink_metadata(&target).is_ok_and(|metadata| metadata.is_dir()) {
            fs::remove_dir(&target).map_err(|error| tree_io::io(&target, error))?;
            if let Some(parent) = target.parent() {
                tree_io::sync(parent)?;
            }
        }
        tree_io::parents(root, &change.path)?;
        let temporary = stage.join(index.to_string());
        match temporary.symlink_metadata() {
            Ok(_) => fs::remove_file(&temporary).map_err(|error| tree_io::io(&temporary, error))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(tree_io::io(&temporary, error)),
        }
        self.write_entry(entry, &temporary)?;
        tree_io::sync(stage)?;
        crate::fault::checkpoint("before-install-rename");
        fs::rename(&temporary, &target).map_err(|error| tree_io::io(&target, error))?;
        tree_io::sync(stage)?;
        if let Some(parent) = target.parent() {
            tree_io::sync(parent)?;
        }
        Ok(())
    }

    fn write_entry(&self, entry: &Entry, temporary: &Path) -> Result<()> {
        let objects = self.objects;
        match entry.kind {
            EntryKind::Symlink => {
                let bytes = objects.read(&entry.object)?;
                let text = std::str::from_utf8(&bytes)
                    .map_err(|_| Error::InvalidSymlink(temporary.display().to_string()))?;
                std::os::unix::fs::symlink(text, temporary)
                    .map_err(|error| tree_io::io(temporary, error))?;
            }
            EntryKind::File | EntryKind::Executable => {
                objects.clone_to(&entry.object, temporary)?;
                fs::set_permissions(
                    temporary,
                    fs::Permissions::from_mode(if entry.kind == EntryKind::Executable {
                        0o755
                    } else {
                        0o644
                    }),
                )
                .map_err(|error| tree_io::io(temporary, error))?;
                let file = File::open(temporary).map_err(|error| tree_io::io(temporary, error))?;
                file.set_modified(std::time::SystemTime::now())
                    .map_err(|error| tree_io::io(temporary, error))?;
                crate::durability::sync_file(&file)
                    .map_err(|error| tree_io::io(temporary, error))?;
            }
        }
        Ok(())
    }
}
fn planned_current(
    root: &Path,
    path: &ResourcePath,
    old: &Snapshot,
    new: &Snapshot,
    limit: u64,
) -> Result<Option<Entry>> {
    for (index, _) in path.as_str().match_indices('/') {
        let parent = ResourcePath::parse(&path.as_str()[..index])?;
        if let Some(previous) = old.get(&parent) {
            if !new.contains_key(&parent) {
                let current = tree_io::read(root, &parent, limit)?.map(|value| value.0);
                if current.as_ref() != Some(previous) {
                    return Err(Error::DirtyPath(parent.as_str().into()));
                }
                return Ok(None);
            }
        }
    }
    tree_io::checked(root, path)?;
    Ok(tree_io::read(root, path, limit)?.map(|value| value.0))
}
fn validate_directory_replacement(
    root: &Path,
    path: &ResourcePath,
    old: &Snapshot,
    new: &Snapshot,
) -> Result<()> {
    let full = root.join(path.as_str());
    if !fs::symlink_metadata(&full).is_ok_and(|meta| meta.is_dir()) {
        return Ok(());
    }
    let mut directories = vec![full];
    while let Some(directory) = directories.pop() {
        let relative =
            directory.strip_prefix(root).map_err(|_| Error::PathConflict(path.as_str().into()))?;
        let prefix = format!(
            "{}/",
            relative.to_str().ok_or_else(|| Error::PathConflict(path.as_str().into()))?
        );
        if !old.keys().any(|name| name.as_str().starts_with(&prefix)) {
            return Err(Error::PathConflict(prefix));
        }
        for entry in fs::read_dir(&directory).map_err(|error| tree_io::io(&directory, error))? {
            let entry = entry.map_err(|error| tree_io::io(&directory, error))?;
            let full = entry.path();
            if entry.file_type().map_err(|error| tree_io::io(&full, error))?.is_dir() {
                directories.push(full);
                continue;
            }
            let relative =
                full.strip_prefix(root).map_err(|_| Error::PathConflict(path.as_str().into()))?;
            let name = ResourcePath::parse(
                relative.to_str().ok_or_else(|| Error::PathConflict(path.as_str().into()))?,
            )?;
            if !old.contains_key(&name) || new.contains_key(&name) {
                return Err(Error::PathConflict(name.as_str().into()));
            }
        }
    }
    Ok(())
}
