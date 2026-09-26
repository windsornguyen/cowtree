// Copyright (c) 2026 Windsor Nguyen

//! Convert verified source entries into the authority's portable manifest.

use std::{fs, os::unix::fs::MetadataExt, path::Path};

use cowtree::{PathClass, TreeEntry, TreeKind};
use cowtree_metadata::{Entry, EntryKind, FileKind, ResourcePath, Snapshot, objects::ObjectId};
use unicode_normalization::UnicodeNormalization;

use crate::{Error, Policy, Result, error::Issue, paths};

pub(crate) fn capture(root: &Path, entries: &[TreeEntry], policy: &Policy) -> Result<Snapshot> {
    let mut manifest = Snapshot::new();
    for entry in entries {
        if entry.classification != PathClass::Source || entry.kind == TreeKind::Directory {
            continue;
        }
        let original = entry.path.to_str().ok_or_else(|| Issue::NonUtf8.at(&entry.path))?;
        if entry.link.as_ref().is_some_and(|link| link.to_str().is_none()) {
            return Err(Issue::NonUtf8.at(&entry.path));
        }
        let name: String = original.nfc().collect();
        if name != original {
            let original = fs::symlink_metadata(root.join(&entry.path))
                .map_err(|error| Error::io(root, error))?;
            let canonical =
                fs::symlink_metadata(root.join(&name)).map_err(|error| Error::io(root, error))?;
            if original.dev() != canonical.dev() || original.ino() != canonical.ino() {
                return Err(Issue::AliasedPath.at(&entry.path));
            }
        }
        let digest =
            entry.digest.as_deref().ok_or_else(|| Issue::IncompleteRecord.at(&entry.path))?;
        let file_kind = match entry.kind {
            TreeKind::Symlink => FileKind::Symlink,
            TreeKind::File if entry.mode & 0o100 != 0 => FileKind::Executable,
            TreeKind::File => FileKind::File,
            TreeKind::Directory => continue,
        };
        let scope =
            policy.pins.iter().map(|pin| pin.path.nfc().collect::<String>()).find(|prefix| {
                name == *prefix
                    || name.strip_prefix(prefix).is_some_and(|suffix| suffix.starts_with('/'))
            });
        let kind = match scope {
            Some(scope) => EntryKind::ReadOnly { file_kind, scope: ResourcePath::parse(scope)? },
            None => match file_kind {
                FileKind::File => EntryKind::File,
                FileKind::Executable => EntryKind::Executable,
                FileKind::Symlink => EntryKind::Symlink,
            },
        };
        manifest
            .insert(ResourcePath::parse(name)?, Entry { object: ObjectId::parse(digest)?, kind });
    }
    paths::aliases(manifest.keys().map(ResourcePath::as_str))?;
    Ok(manifest)
}

pub(crate) fn verify_dependencies(actual: &Snapshot, expected: &Snapshot) -> Result<()> {
    for path in actual.keys().chain(expected.keys()) {
        let before = expected.get(path);
        let after = actual.get(path);
        if [before, after].into_iter().flatten().any(|entry| entry.kind.read_only_scope().is_some())
            && before != after
        {
            return Err(Issue::DependencyChanged.at(Path::new(path.as_str())));
        }
    }
    Ok(())
}
