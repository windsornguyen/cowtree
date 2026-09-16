// Copyright (c) 2026 Windsor Nguyen

//! Bootstrap a durable source snapshot with one manifest pin before any payload writes.

use crate::{EntryKind, Error, ResourcePath, Result, Snapshot, Store, Version, objects::ObjectId};
use crate::{database::read_tip, tree_io};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use std::{collections::BTreeSet, fs, path::Path};

impl Store {
    /// Import one quiescent Git-tracked source tree. Repeating an identical import recovers it.
    pub fn import_tree(
        &mut self,
        source: &Path,
        paths: &[ResourcePath],
    ) -> Result<(Version, ObjectId)> {
        let _lock = self.filesystem_lock()?;
        let source = fs::canonicalize(source).map_err(|error| tree_io::io(source, error))?;
        if tree_io::identity(&source)?.0 != tree_io::identity(&self.root)?.0 {
            return Err(Error::InstallConflict(
                "source and authority must share a filesystem".into(),
            ));
        }
        tree_io::probe_clone(&self.root)?;
        let unique: BTreeSet<_> = paths.iter().cloned().collect();
        if unique.len() != paths.len() || unique.len() > self.limits.max_paths as usize {
            return Err(Error::ImportConflict);
        }
        let mut snapshot = Snapshot::new();
        for path in &unique {
            let (entry, _) = tree_io::read(&source, path, self.limits.max_object_bytes)?
                .ok_or_else(|| Error::TreeChanged(path.as_str().into()))?;
            snapshot.insert(path.clone(), entry);
        }
        tree_io::validate_names(&self.root, &snapshot)?;
        let bytes = serde_json::to_vec(&snapshot)?;
        let root = ObjectId::from_bytes(&bytes);
        let paths_json = serde_json::to_string(&unique)?;
        if self.pin_import(&source, &paths_json, &root)? {
            return Ok((Version::new(1)?, root));
        }
        self.objects.put(&bytes)?;
        crate::fault::checkpoint("after-import-manifest");
        for (path, expected) in &snapshot {
            let (entry, bytes) = tree_io::read(&source, path, self.limits.max_object_bytes)?
                .ok_or_else(|| Error::TreeChanged(path.as_str().into()))?;
            if entry != *expected {
                return Err(Error::TreeChanged(path.as_str().into()));
            }
            match entry.kind {
                EntryKind::Symlink => {
                    self.objects.put(&bytes)?;
                }
                EntryKind::File | EntryKind::Executable => {
                    self.objects.put_file(&source.join(path.as_str()), &entry.object)?;
                }
            }
        }
        crate::fault::checkpoint("after-import-payloads");
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (tip, _) = read_tip(&tx)?;
        if tip != 0 {
            return Err(Error::ImportConflict);
        }
        tx.execute("INSERT INTO epochs VALUES(1,?1)", [root.as_str()])?;
        tx.execute("UPDATE settings SET tip=1 WHERE singleton=1", [])?;
        tx.execute("UPDATE imports SET ready=1 WHERE singleton=1", [])?;
        tx.commit()?;
        Ok((Version::new(1)?, root))
    }
    fn pin_import(&mut self, source: &Path, paths: &str, root: &ObjectId) -> Result<bool> {
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous: Option<(String, String, String, bool)> = tx
            .query_row("SELECT source,paths,root,ready FROM imports WHERE singleton=1", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .optional()?;
        let source = source.to_str().ok_or(Error::ImportConflict)?;
        if let Some((old_source, old_paths, old_root, ready)) = previous {
            if old_source != source || old_paths != paths || old_root != root.as_str() {
                return Err(Error::ImportConflict);
            }
            return Ok(ready);
        }
        let count: i64 = tx.query_row("SELECT count(*) FROM leaves", [], |row| row.get(0))?;
        if count != 0 || read_tip(&tx)?.0 != 0 {
            return Err(Error::ImportConflict);
        }
        tx.execute(
            "INSERT INTO imports VALUES(1,?1,?2,?3,0)",
            params![source, paths, root.as_str()],
        )?;
        tx.commit()?;
        Ok(false)
    }
}
