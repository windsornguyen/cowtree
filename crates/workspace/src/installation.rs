// Copyright (c) 2026 Windsor Nguyen

//! Replay per-file replacements from retained before and after images.
//!
//! 1. Check the final namespace before writing intent.
//! 2. Flush both images before publishing the installation record.
//! 3. Replay only when current bytes match the before or after image.
//! 4. Retire images after the caller has durably reconciled authority state.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

use cowtree_metadata::{Entry, FileKind, ResourcePath};
use serde::{Deserialize, Serialize};

use crate::{Error, Result, error::Issue, images, paths, records};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Change {
    /// Normalized path relative to the owned leaf.
    pub path: ResourcePath,
    /// Physical image allowed before replay.
    pub before: Option<Entry>,
    /// Physical image promised after replay.
    pub after: Option<Entry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InstallRecord {
    /// Absolute directory that owns the changed names.
    pub root: PathBuf,
    /// Complete ordered change set with unique paths.
    pub changes: Vec<Change>,
    /// Device checked again before applying any replacement.
    #[serde(default)]
    pub device: u64,
    /// Directory inode checked together with the device.
    #[serde(default)]
    pub inode: u64,
}

pub(crate) struct Installation {
    /// Private journal containing the retained images and durable intent.
    pub directory: PathBuf,
    /// Complete replay contract read from install.json.
    pub record: InstallRecord,
}

#[derive(Clone, Copy)]
pub(crate) enum Direction {
    Apply,
    Rollback,
}

impl Installation {
    pub fn open(directory: &Path) -> Result<Self> {
        Ok(Self {
            directory: directory.into(),
            record: records::read(&directory.join("install.json"))?,
        })
    }

    pub fn preflight(record: &InstallRecord) -> Result<()> {
        let root = &record.root;
        let metadata = fs::symlink_metadata(root).map_err(|error| Error::io(root, error))?;
        if !root.is_absolute() || !metadata.is_dir() {
            return Err(Issue::InvalidRoot.at(root));
        }
        let changes: BTreeMap<_, _> =
            record.changes.iter().map(|change| (&change.path, change)).collect();
        if changes.len() != record.changes.len() {
            return Err(Issue::InvalidPath.at(root));
        }
        let deleted: BTreeSet<_> = record
            .changes
            .iter()
            .filter(|change| change.before.is_some() && change.after.is_none())
            .map(|change| change.path.as_str())
            .collect();
        for change in &record.changes {
            let target = paths::local(root, change.path.as_str())?;
            if !images::same(images::fingerprint(&target)?.as_ref(), change.before.as_ref()) {
                return Err(Issue::SourceChanged.at(&target));
            }
            if change.after.is_none() {
                continue;
            }
            for parent in target.ancestors().skip(1).take_while(|parent| *parent != root) {
                let relative =
                    parent.strip_prefix(root).map_err(|_| Issue::InvalidPath.at(parent))?;
                let name = ResourcePath::parse(
                    relative.to_str().ok_or_else(|| Issue::NonUtf8.at(parent))?,
                )?;
                match changes.get(&name) {
                    Some(planned) if planned.after.is_some() => {
                        return Err(Issue::SourceChanged.at(parent));
                    }
                    None if paths::exists(parent)? && !parent.is_dir() => {
                        return Err(Issue::SourceChanged.at(parent));
                    }
                    _ => {}
                }
            }
            if target.is_dir() && !target.is_symlink() {
                validate_removal(&target, root, &deleted)?;
            }
        }
        Ok(())
    }

    pub fn prepare(directory: &Path, mut record: InstallRecord, objects: &Path) -> Result<Self> {
        Self::preflight(&record)?;
        let identity =
            fs::metadata(&record.root).map_err(|error| Error::io(&record.root, error))?;
        let parent = directory.parent().ok_or_else(|| Issue::InvalidRoot.at(directory))?;
        if identity.dev() != fs::metadata(parent).map_err(|error| Error::io(parent, error))?.dev() {
            return Err(Error::io(directory, std::io::ErrorKind::CrossesDevices.into()));
        }
        record.device = identity.dev();
        record.inode = identity.ino();
        fs::create_dir(directory).map_err(|error| Error::io(directory, error))?;
        let install = Self { directory: directory.into(), record };
        if let Err(original) = install.capture(objects) {
            if let Err(cleanup) = records::remove_directory(directory) {
                return Err(Error::Cleanup {
                    original: Box::new(original),
                    cleanup: Box::new(cleanup),
                });
            }
            return Err(original);
        }
        Ok(install)
    }

    fn capture(&self, objects: &Path) -> Result<()> {
        for (index, change) in self.record.changes.iter().enumerate() {
            let target = paths::local(&self.record.root, change.path.as_str())?;
            if !images::same(images::fingerprint(&target)?.as_ref(), change.before.as_ref()) {
                return Err(Issue::SourceChanged.at(&target));
            }
            if change.before == change.after {
                continue;
            }
            if let Some(before) = &change.before {
                images::save(&target, &self.directory.join(format!("before-{index}")), before)?;
            }
            if let Some(after) = &change.after {
                images::stage(
                    &objects.join(after.object.as_str()),
                    &self.directory.join(format!("after-{index}")),
                    after,
                )?;
            }
        }
        records::sync_directory(&self.directory)?;
        records::write(&self.directory.join("install.json"), &self.record)?;
        records::sync_directory(
            self.directory.parent().ok_or_else(|| Issue::InvalidRoot.at(&self.directory))?,
        )
    }

    pub fn apply(&self, direction: Direction) -> Result<()> {
        let identity = fs::symlink_metadata(&self.record.root)
            .map_err(|error| Error::io(&self.record.root, error))?;
        if !identity.is_dir()
            || (identity.dev(), identity.ino()) != (self.record.device, self.record.inode)
        {
            return Err(Issue::ChangedDirectory.at(&self.record.root));
        }
        let mut operations: Vec<_> = self.record.changes.iter().enumerate().collect();
        operations.sort_by_key(|(_, change)| {
            let desired = match direction {
                Direction::Apply => &change.after,
                Direction::Rollback => &change.before,
            };
            let depth = change.path.as_str().split('/').count();
            (desired.is_some(), if desired.is_none() { usize::MAX - depth } else { depth })
        });
        for (index, change) in operations {
            self.replace(index, change, direction)?;
        }
        Ok(())
    }

    fn replace(&self, index: usize, change: &Change, direction: Direction) -> Result<()> {
        let (expected, desired, image, temporary) = match direction {
            Direction::Apply => (&change.before, &change.after, "after", "apply"),
            Direction::Rollback => (&change.after, &change.before, "before", "rollback"),
        };
        let root = &self.record.root;
        let target = paths::local(root, change.path.as_str())?;
        let current = images::fingerprint(&target)?;
        if images::same(current.as_ref(), desired.as_ref()) {
            if desired.as_ref().is_some_and(|entry| entry.kind.file_kind() != FileKind::Symlink) {
                images::sync(&target)?;
            }
            let parent = target
                .ancestors()
                .skip(1)
                .find(|parent| parent.is_dir())
                .ok_or_else(|| Issue::InvalidRoot.at(root))?;
            return records::sync_directory(parent);
        }
        if !images::same(current.as_ref(), expected.as_ref()) {
            return Err(Issue::SourceChanged.at(&target));
        }
        let parent = target.parent().ok_or_else(|| Issue::InvalidPath.at(&target))?;
        let Some(desired) = desired else {
            fs::remove_file(&target).map_err(|error| Error::io(&target, error))?;
            records::sync_directory(parent)?;
            return prune_parents(parent, root);
        };
        create_parents(parent, root)?;
        remove_empty_tree(&target)?;
        let image = self.directory.join(format!("{image}-{index}"));
        let temporary = self.directory.join(format!("{temporary}-{index}"));
        if !paths::exists(&temporary)? {
            images::save(&image, &temporary, desired)?;
            records::sync_directory(&self.directory)?;
        }
        if !images::same(images::fingerprint(&temporary)?.as_ref(), Some(desired)) {
            return Err(Issue::CorruptObject.at(&temporary));
        }
        fs::rename(&temporary, &target).map_err(|error| Error::io(&target, error))?;
        records::sync_directory(parent)?;
        records::sync_directory(&self.directory)
    }

    pub fn finish(self) -> Result<()> {
        records::remove_directory(&self.directory)
    }
}

fn validate_removal(directory: &Path, root: &Path, deleted: &BTreeSet<&str>) -> Result<()> {
    let relative = directory
        .strip_prefix(root)
        .map_err(|_| Issue::InvalidPath.at(directory))?
        .to_str()
        .ok_or_else(|| Issue::NonUtf8.at(directory))?;
    let prefix = format!("{relative}/");
    if !deleted.iter().any(|name| name.starts_with(&prefix)) {
        return Err(Issue::SourceChanged.at(directory));
    }
    for child in paths::entries(directory)? {
        if child.is_dir() && !child.is_symlink() {
            validate_removal(&child, root, deleted)?;
            continue;
        }
        let name = child
            .strip_prefix(root)
            .map_err(|_| Issue::InvalidPath.at(&child))?
            .to_str()
            .ok_or_else(|| Issue::NonUtf8.at(&child))?;
        if !deleted.contains(name) {
            return Err(Issue::SourceChanged.at(&child));
        }
    }
    Ok(())
}

fn prune_parents(mut path: &Path, root: &Path) -> Result<()> {
    while path != root {
        match fs::remove_dir(path) {
            Ok(()) => {}
            Err(error)
                if error.kind() == std::io::ErrorKind::DirectoryNotEmpty
                    || error.kind() == std::io::ErrorKind::AlreadyExists =>
            {
                return Ok(());
            }
            Err(error) => return Err(Error::io(path, error)),
        }
        path = path.parent().ok_or_else(|| Issue::InvalidPath.at(path))?;
        records::sync_directory(path)?;
    }
    Ok(())
}

fn create_parents(path: &Path, root: &Path) -> Result<()> {
    let mut current = root.to_path_buf();
    for part in path.strip_prefix(root).map_err(|_| Issue::InvalidPath.at(path))?.components() {
        let parent = current.clone();
        current.push(part);
        if !current.exists() {
            fs::create_dir(&current).map_err(|error| Error::io(&current, error))?;
            records::sync_directory(&parent)?;
        }
    }
    Ok(())
}

fn remove_empty_tree(path: &Path) -> Result<()> {
    if !path.is_dir() || path.is_symlink() {
        return Ok(());
    }
    for child in paths::entries(path)? {
        remove_empty_tree(&child)?;
    }
    fs::remove_dir(path).map_err(|error| Error::io(path, error))?;
    records::sync_directory(path.parent().ok_or_else(|| Issue::InvalidPath.at(path))?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cowtree_metadata::{EntryKind, objects::ObjectId};
    use std::os::unix::fs::PermissionsExt;

    type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

    fn setup() -> std::result::Result<Option<tempfile::TempDir>, Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let supported = cowtree::inspect_path(directory.path())?.supported();
        if std::env::var_os("COWTREE_EXPECT_SUPPORTED").is_some_and(|value| value == "1") {
            assert!(supported);
        }
        if !supported {
            eprintln!("skip native filesystem");
            return Ok(None);
        }
        fs::create_dir(directory.path().join("leaf"))?;
        fs::create_dir(directory.path().join("objects"))?;
        Ok(Some(directory))
    }

    fn entry(bytes: &[u8]) -> Entry {
        Entry { object: ObjectId::from_bytes(bytes), kind: EntryKind::File }
    }

    fn record(root: &Path) -> Result<InstallRecord> {
        Ok(InstallRecord {
            root: root.into(),
            changes: vec![Change {
                path: ResourcePath::parse("file")?,
                before: Some(entry(b"before")),
                after: Some(entry(b"after")),
            }],
            device: 0,
            inode: 0,
        })
    }

    fn stage(directory: &Path) -> TestResult {
        let source = directory.join("leaf/file");
        fs::write(&source, b"before")?;
        fs::set_permissions(source, fs::Permissions::from_mode(0o644))?;
        fs::write(directory.join("objects").join(entry(b"after").object.as_str()), b"after")?;
        Ok(())
    }

    #[test]
    fn replay_preserves_edits_made_after_installation() -> TestResult {
        let Some(root) = setup()? else {
            return Ok(());
        };
        stage(root.path())?;
        let leaf = root.path().join("leaf");
        let journal = root.path().join("journal");
        let install =
            Installation::prepare(&journal, record(&leaf)?, &root.path().join("objects"))?;
        install.apply(Direction::Apply)?;
        Installation::open(&journal)?.apply(Direction::Apply)?;
        assert_eq!(fs::read(leaf.join("file"))?, b"after");
        fs::write(leaf.join("file"), b"private edit")?;
        assert!(matches!(
            install.apply(Direction::Rollback),
            Err(Error::State { issue: Issue::SourceChanged, .. })
        ));
        assert_eq!(fs::read(leaf.join("file"))?, b"private edit");
        assert!(journal.join("before-0").exists());
        Ok(())
    }

    #[test]
    fn rollback_restores_bytes_after_an_unacknowledged_apply() -> TestResult {
        let Some(root) = setup()? else {
            return Ok(());
        };
        stage(root.path())?;
        let leaf = root.path().join("leaf");
        let journal = root.path().join("journal");
        Installation::prepare(&journal, record(&leaf)?, &root.path().join("objects"))?
            .apply(Direction::Apply)?;
        let recovered = Installation::open(&journal)?;
        recovered.apply(Direction::Rollback)?;
        recovered.apply(Direction::Rollback)?;
        assert_eq!(fs::read(leaf.join("file"))?, b"before");
        recovered.finish()?;
        assert!(!journal.exists());
        Ok(())
    }

    #[test]
    fn a_replaced_leaf_cannot_receive_journal_writes() -> TestResult {
        let Some(root) = setup()? else {
            return Ok(());
        };
        stage(root.path())?;
        let leaf = root.path().join("leaf");
        let journal = root.path().join("journal");
        let install =
            Installation::prepare(&journal, record(&leaf)?, &root.path().join("objects"))?;
        fs::rename(&leaf, root.path().join("original"))?;
        fs::create_dir(&leaf)?;
        fs::write(leaf.join("file"), b"unrelated owner")?;
        assert!(matches!(
            install.apply(Direction::Apply),
            Err(Error::State { issue: Issue::ChangedDirectory, .. })
        ));
        assert_eq!(fs::read(leaf.join("file"))?, b"unrelated owner");
        assert_eq!(fs::read(root.path().join("original/file"))?, b"before");
        Ok(())
    }

    #[test]
    fn namespace_conflicts_are_rejected_before_intent_is_written() -> TestResult {
        let Some(root) = setup()? else {
            return Ok(());
        };
        stage(root.path())?;
        let leaf = root.path().join("leaf");
        let journal = root.path().join("journal");
        let mut request = record(&leaf)?;
        request.changes[0].path = ResourcePath::parse("file/child")?;
        request.changes[0].before = None;
        assert!(matches!(
            Installation::prepare(&journal, request, &root.path().join("objects")),
            Err(Error::State { issue: Issue::SourceChanged, .. })
        ));
        assert!(!journal.exists());
        assert_eq!(fs::read(leaf.join("file"))?, b"before");
        Ok(())
    }
}
