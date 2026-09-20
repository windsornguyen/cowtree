// Copyright (c) 2026 Windsor Nguyen

//! Initial imports publish once, retain unfinished bytes, and resume from durable progress.
#![allow(clippy::unwrap_used)]

use std::{collections::BTreeMap, fs};

use cowtree_metadata::objects::ObjectId;
use cowtree_metadata::{Entry, EntryKind, Error, Limits, ResourcePath, Snapshot, Store, Version};

fn source(directory: &std::path::Path, count: usize) -> Snapshot {
    fs::create_dir(directory).unwrap();
    (0..count)
        .map(|index| {
            let name = format!("file-{index:03}");
            let bytes = format!("payload-{index}");
            fs::write(directory.join(&name), &bytes).unwrap();
            (
                ResourcePath::parse(name).unwrap(),
                Entry { object: ObjectId::from_bytes(bytes.as_bytes()), kind: EntryKind::File },
            )
        })
        .collect()
}

#[test]
fn progress_survives_reopen_and_collection_without_publishing_a_partial_snapshot() {
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path().join("source");
    let manifest = source(&directory, 70);
    let root = temporary.path().join("store");
    let mut store = Store::create(&root, Limits::default()).unwrap();
    assert_eq!(store.status_import().unwrap(), None);
    let begin = store.begin_import(manifest.clone()).unwrap();
    assert_eq!(begin.completed, 0);
    assert_eq!(begin.total, 70);
    assert!(!begin.complete);
    let chunk = store.import_chunk(&begin.root, &directory).unwrap();
    assert_eq!(chunk.completed, 64);
    assert_eq!(store.tip().unwrap().0.get(), 0);
    assert!(matches!(store.finish_import(&begin.root), Err(Error::ImportNotReady)));
    assert!(matches!(store.create_leaf(), Err(Error::ImportConflict(_))));
    drop(store);
    let mut store = Store::open(&root).unwrap();
    assert_eq!(store.status_import().unwrap(), Some(chunk.clone()));
    assert_eq!(store.begin_import(manifest.clone()).unwrap(), chunk);
    assert_eq!(store.maintain().unwrap().objects_removed, 0);
    let final_chunk = store.import_chunk(&begin.root, &directory).unwrap();
    assert_eq!(final_chunk.completed, 70);
    assert!(!final_chunk.complete);
    let finished = store.finish_import(&begin.root).unwrap();
    assert!(finished.complete);
    assert_eq!(store.tip().unwrap(), (Version::new(1).unwrap(), begin.root.clone()));
    assert_eq!(store.snapshot(Version::new(1).unwrap()).unwrap(), manifest);
    for (path, entry) in &manifest {
        let actual = store.read(Version::new(1).unwrap(), path).unwrap().unwrap();
        assert_eq!(ObjectId::from_bytes(&actual), entry.object);
    }
    assert_eq!(store.finish_import(&begin.root).unwrap(), finished);
    assert_eq!(store.begin_import(manifest).unwrap(), finished);
    assert!(store.create_leaf().is_ok());
}

#[test]
fn import_identity_and_source_bytes_cannot_change_during_resume() {
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path().join("source");
    let manifest = source(&directory, 2);
    let mut store = Store::create(&temporary.path().join("store"), Limits::default()).unwrap();
    let begin = store.begin_import(manifest).unwrap();
    assert!(matches!(store.begin_import(BTreeMap::new()), Err(Error::ImportConflict(_))));
    fs::write(directory.join("file-001"), b"changed").unwrap();
    assert!(matches!(
        store.import_chunk(&begin.root, &directory),
        Err(Error::ImportSourceChanged(_))
    ));
    assert_eq!(store.status_import().unwrap().unwrap().completed, 0);
    assert_eq!(store.tip().unwrap().0.get(), 0);
    fs::write(directory.join("file-001"), b"payload-1").unwrap();
    assert_eq!(store.import_chunk(&begin.root, &directory).unwrap().completed, 2);
    store.finish_import(&begin.root).unwrap();
}

#[test]
fn imports_cannot_overwrite_an_authority_that_has_allocated_leaf_identities() {
    let temporary = tempfile::tempdir().unwrap();
    let mut store = Store::create(&temporary.path().join("store"), Limits::default()).unwrap();
    let leaf = store.create_leaf().unwrap();
    store.drop_leaf(leaf).unwrap();
    assert!(matches!(store.begin_import(BTreeMap::new()), Err(Error::ImportConflict(_))));
}

#[test]
fn empty_import_still_commits_exactly_one_initial_epoch() {
    let temporary = tempfile::tempdir().unwrap();
    let mut store = Store::create(&temporary.path().join("store"), Limits::default()).unwrap();
    let begin = store.begin_import(BTreeMap::new()).unwrap();
    assert_eq!(begin.total, 0);
    assert!(store.finish_import(&begin.root).unwrap().complete);
    assert_eq!(store.tip().unwrap().0.get(), 1);
}

#[test]
fn finish_refuses_corrupt_completed_objects_and_preserves_epoch_zero() {
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path().join("source");
    let manifest = source(&directory, 2);
    let root = temporary.path().join("store");
    let mut store = Store::create(&root, Limits::default()).unwrap();
    let begin = store.begin_import(manifest.clone()).unwrap();
    store.import_chunk(&begin.root, &directory).unwrap();
    let object = &manifest.values().next().unwrap().object;
    fs::write(root.join("objects").join(object.as_str()), b"corrupt").unwrap();
    assert!(matches!(
        store.finish_import(&begin.root),
        Err(Error::Object(cowtree_metadata::objects::ObjectError::Corrupt { .. }))
    ));
    assert_eq!(store.tip().unwrap().0.get(), 0);
    assert!(!store.status_import().unwrap().unwrap().complete);
}

#[test]
fn chunks_obey_byte_budget_and_maximum_object_size() {
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path().join("source");
    fs::create_dir(&directory).unwrap();
    let bytes = vec![42; 12 * 1024 * 1024];
    let manifest: Snapshot = (0..3)
        .map(|index| {
            let name = format!("file-{index}");
            fs::write(directory.join(&name), &bytes).unwrap();
            (
                ResourcePath::parse(name).unwrap(),
                Entry { object: ObjectId::from_bytes(&bytes), kind: EntryKind::File },
            )
        })
        .collect();
    let mut store = Store::create(&temporary.path().join("store"), Limits::default()).unwrap();
    let begin = store.begin_import(manifest.clone()).unwrap();
    assert_eq!(store.import_chunk(&begin.root, &directory).unwrap().completed, 2);
    assert_eq!(store.import_chunk(&begin.root, &directory).unwrap().completed, 3);
    store.finish_import(&begin.root).unwrap();
    let limits = Limits { max_object_bytes: 1_024, ..Limits::default() };
    let mut small = Store::create(&temporary.path().join("small"), limits).unwrap();
    let begin = small.begin_import(manifest).unwrap();
    assert!(matches!(
        small.import_chunk(&begin.root, &directory),
        Err(Error::Limit(cowtree_metadata::LimitKind::ObjectBytes))
    ));
    assert_eq!(small.status_import().unwrap().unwrap().completed, 0);
}

#[test]
fn symlinks_are_imported_as_link_text_without_following_parent_links_or_hardlinks() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path().join("source");
    let mut manifest = source(&directory, 1);
    symlink("file-000", directory.join("link")).unwrap();
    manifest.insert(
        ResourcePath::parse("link").unwrap(),
        Entry { object: ObjectId::from_bytes(b"file-000"), kind: EntryKind::Symlink },
    );
    fs::set_permissions(directory.join("file-000"), fs::Permissions::from_mode(0o755)).unwrap();
    manifest.get_mut(&ResourcePath::parse("file-000").unwrap()).unwrap().kind =
        EntryKind::Executable;
    let mut store = Store::create(&temporary.path().join("store"), Limits::default()).unwrap();
    let begin = store.begin_import(manifest.clone()).unwrap();
    store.import_chunk(&begin.root, &directory).unwrap();
    store.finish_import(&begin.root).unwrap();
    assert_eq!(store.snapshot(Version::new(1).unwrap()).unwrap(), manifest);

    let mut hardlinked =
        Store::create(&temporary.path().join("hardlinked"), Limits::default()).unwrap();
    let begin = hardlinked.begin_import(manifest).unwrap();
    fs::hard_link(directory.join("file-000"), directory.join("hardlink")).unwrap();
    assert!(matches!(
        hardlinked.import_chunk(&begin.root, &directory),
        Err(Error::ImportSourceChanged(_))
    ));
    fs::remove_file(directory.join("hardlink")).unwrap();
    fs::set_permissions(directory.join("file-000"), fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(
        hardlinked.import_chunk(&begin.root, &directory),
        Err(Error::ImportSourceChanged(_))
    ));

    symlink(&directory, directory.join("parent")).unwrap();
    let manifest = BTreeMap::from([(
        ResourcePath::parse("parent/file-000").unwrap(),
        Entry { object: ObjectId::from_bytes(b"payload-0"), kind: EntryKind::File },
    )]);
    let mut parent =
        Store::create(&temporary.path().join("parent-store"), Limits::default()).unwrap();
    let begin = parent.begin_import(manifest).unwrap();
    assert!(parent.import_chunk(&begin.root, &directory).is_err());
    assert_eq!(parent.status_import().unwrap().unwrap().completed, 0);
}

#[test]
fn completed_import_receipt_does_not_pin_expired_payloads_forever() {
    use cowtree_metadata::{ProposalInput, RequestId};
    use std::collections::BTreeSet;
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path().join("source");
    let manifest = source(&directory, 1);
    let root = temporary.path().join("store");
    let limits = Limits { retained_epochs: 1, ..Limits::default() };
    let mut store = Store::create(&root, limits).unwrap();
    let imported = manifest.values().next().unwrap().object.clone();
    let begin = store.begin_import(manifest).unwrap();
    store.import_chunk(&begin.root, &directory).unwrap();
    let finished = store.finish_import(&begin.root).unwrap();
    let leaf = store.create_leaf().unwrap();
    let paths = BTreeSet::from([ResourcePath::parse("file-000").unwrap()]);
    let grant = store.acquire(leaf, &paths).unwrap().remove(0);
    let grant = store.activate(&grant).unwrap();
    let object = store.stage(leaf, b"replacement").unwrap();
    store.edit(&grant, Some(Entry { object, kind: EntryKind::File })).unwrap();
    let request = RequestId { leaf, sequence: 1 };
    store.propose(ProposalInput { request, paths }).unwrap();
    let candidate = store.prepare(request).unwrap();
    store.commit(candidate).unwrap();
    store.drop_leaf(leaf).unwrap();
    store.maintain().unwrap();
    assert!(!root.join("objects").join(imported.as_str()).exists());
    assert_eq!(store.finish_import(&begin.root).unwrap(), finished);
    assert_eq!(store.tip().unwrap().0.get(), 2);
}

#[test]
fn imported_executable_kind_uses_gits_owner_execute_bit() {
    use std::os::unix::fs::PermissionsExt;
    for mode in [0o645, 0o654] {
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().join("source");
        let manifest = source(&directory, 1);
        fs::set_permissions(directory.join("file-000"), fs::Permissions::from_mode(mode)).unwrap();
        let mut store = Store::create(&temporary.path().join("store"), Limits::default()).unwrap();
        let begin = store.begin_import(manifest.clone()).unwrap();
        store.import_chunk(&begin.root, &directory).unwrap();
        store.finish_import(&begin.root).unwrap();
        assert_eq!(store.snapshot(Version::new(1).unwrap()).unwrap(), manifest);
    }
}

#[test]
fn collection_refuses_an_import_manifest_that_disagrees_with_its_bound_identity() {
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path().join("source");
    let manifest = source(&directory, 1);
    let root = temporary.path().join("store");
    let mut store = Store::create(&root, Limits::default()).unwrap();
    let begin = store.begin_import(manifest.clone()).unwrap();
    store.import_chunk(&begin.root, &directory).unwrap();
    let connection = rusqlite::Connection::open(root.join("metadata.sqlite3")).unwrap();
    connection.execute("UPDATE imports SET manifest_json='{}'", []).unwrap();
    assert!(matches!(store.maintain(), Err(Error::Schema)));
    assert!(matches!(store.status_import(), Err(Error::Schema)));
    let imported = &manifest.values().next().unwrap().object;
    assert!(root.join("objects").join(imported.as_str()).exists());
    assert_eq!(store.tip().unwrap().0.get(), 0);
}
