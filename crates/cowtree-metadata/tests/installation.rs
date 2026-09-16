// Copyright (c) 2026 Windsor Nguyen

#![allow(clippy::unwrap_used)]
//! Physical namespace changes must preserve unrelated bytes and recover before acknowledgement.

use cowtree_metadata::{
    Entry, EntryKind, LeafId, Limits, ProposalInput, RequestId, ResourcePath, Store, Version,
};
use std::{collections::BTreeSet, fs, path::Path};

fn path(name: &str) -> ResourcePath {
    ResourcePath::parse(name).unwrap()
}
fn publish(
    store: &mut Store,
    leaf: LeafId,
    sequence: u64,
    changes: &[(&str, Option<&[u8]>)],
) -> Version {
    let paths: BTreeSet<_> = changes.iter().map(|(name, _)| path(name)).collect();
    for grant in store.acquire(leaf, &paths).unwrap() {
        let grant = store.activate(&grant).unwrap();
        let content = changes.iter().find(|(name, _)| *name == grant.path.as_str()).unwrap().1;
        let value = content.map(|bytes| Entry {
            object: store.stage(leaf, bytes).unwrap(),
            kind: EntryKind::File,
        });
        store.edit(&grant, value).unwrap();
    }
    let request = RequestId { leaf, sequence };
    store.propose(ProposalInput { request, paths }).unwrap();
    let candidate = store.prepare(request).unwrap();
    store.commit(candidate).unwrap().version
}
fn fixture(root: &Path) -> (Store, LeafId, LeafId, std::path::PathBuf) {
    let mut store = Store::create(&root.join("store"), Limits::default()).unwrap();
    let writer = store.create_leaf().unwrap();
    let reader = store.create_leaf().unwrap();
    let tree = root.join("tree");
    fs::create_dir(&tree).unwrap();
    store.bind_tree(reader, &tree, Version::new(0).unwrap()).unwrap();
    (store, writer, reader, tree)
}
#[test]
fn tracked_file_and_directory_transitions_install_without_overwriting_untracked_files() {
    let temp = tempfile::tempdir().unwrap();
    let (mut store, writer, reader, tree) = fixture(temp.path());
    let first = publish(&mut store, writer, 1, &[("node", Some(b"file"))]);
    store.install(reader, first).unwrap();
    let second = publish(&mut store, writer, 2, &[("node", None), ("node/child", Some(b"nested"))]);
    store.install(reader, second).unwrap();
    assert_eq!(fs::read(tree.join("node/child")).unwrap(), b"nested");
    fs::write(tree.join("node/untracked"), b"keep").unwrap();
    let third = publish(&mut store, writer, 3, &[("node/child", None), ("node", Some(b"again"))]);
    assert!(store.install(reader, third).is_err());
    assert!(store.binding(reader).unwrap().pending.is_none());
    assert_eq!(fs::read(tree.join("node/untracked")).unwrap(), b"keep");
    fs::remove_file(tree.join("node/untracked")).unwrap();
    store.install(reader, third).unwrap();
    assert_eq!(fs::read(tree.join("node")).unwrap(), b"again");
}
#[test]
fn authorities_and_bound_trees_cannot_overlap_or_alias() {
    let temp = tempfile::tempdir().unwrap();
    let (mut store, _, reader, tree) = fixture(temp.path());
    let other = store.create_leaf().unwrap();
    fs::create_dir(tree.join("nested")).unwrap();
    assert!(store.bind_tree(other, &tree.join("nested"), Version::new(0).unwrap()).is_err());
    assert!(store.bind_tree(other, temp.path(), Version::new(0).unwrap()).is_err());
    assert!(store.bind_tree(other, &temp.path().join("store"), Version::new(0).unwrap()).is_err());
    assert_eq!(store.binding(reader).unwrap().path, fs::canonicalize(tree).unwrap());
}

#[test]
fn identical_untracked_bytes_do_not_silently_become_managed_paths() {
    let temp = tempfile::tempdir().unwrap();
    let (mut store, writer, reader, tree) = fixture(temp.path());
    fs::write(tree.join("new"), b"same").unwrap();
    let version = publish(&mut store, writer, 1, &[("new", Some(b"same"))]);
    assert!(store.install(reader, version).is_err());
    assert_eq!(store.binding(reader).unwrap().version.get(), 0);
    assert!(store.binding(reader).unwrap().pending.is_none());
    assert_eq!(fs::read(tree.join("new")).unwrap(), b"same");
}

#[test]
fn raw_postcapture_deletion_is_not_overwritten_by_installation() {
    let temp = tempfile::tempdir().unwrap();
    let (mut store, _, leaf, tree) = fixture(temp.path());
    let paths = BTreeSet::from([path("new")]);
    let grant = store.acquire(leaf, &paths).unwrap().remove(0);
    let grant = store.activate(&grant).unwrap();
    fs::write(tree.join("new"), b"captured").unwrap();
    store.capture_files(&[grant]).unwrap();
    let request = RequestId { leaf, sequence: 1 };
    store.propose(ProposalInput { request, paths: paths.clone() }).unwrap();
    let candidate = store.prepare(request).unwrap();
    fs::remove_file(tree.join("new")).unwrap();
    let receipt = store.commit(candidate).unwrap();
    assert!(store.install(leaf, receipt.version).is_err());
    assert!(!tree.join("new").exists());
    assert!(store.binding(leaf).unwrap().pending.is_none());
    let grant = store.acquire(leaf, &paths).unwrap().remove(0);
    store.capture_files(&[grant]).unwrap();
    store.install(leaf, receipt.version).unwrap();
    assert!(!tree.join("new").exists());
    assert!(store.view(leaf).unwrap()[0].dirty());
}

#[test]
fn installed_changed_content_does_not_inherit_old_object_modification_times() {
    use std::time::{Duration, SystemTime};
    let temp = tempfile::tempdir().unwrap();
    let (mut store, writer, reader, tree) = fixture(temp.path());
    let first = publish(&mut store, writer, 1, &[("file", Some(b"first"))]);
    store.install(reader, first).unwrap();
    let next = publish(&mut store, writer, 2, &[("file", Some(b"replacement"))]);
    let snapshot = store.snapshot(next).unwrap();
    let object = &snapshot.get(&path("file")).unwrap().object;
    let root = fs::canonicalize(temp.path()).unwrap();
    let immutable = fs::File::open(root.join("store/objects").join(object.as_str())).unwrap();
    immutable.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1)).unwrap();
    let before = SystemTime::now();
    store.install(reader, next).unwrap();
    assert!(fs::metadata(tree.join("file")).unwrap().modified().unwrap() >= before);
}

#[test]
fn unrelated_untracked_names_cannot_collide_with_installer_scratch_space() {
    let temp = tempfile::tempdir().unwrap();
    let (mut store, writer, reader, tree) = fixture(temp.path());
    let version = publish(&mut store, writer, 1, &[("file", Some(b"published"))]);
    let db = rusqlite::Connection::open(temp.path().join("store/metadata.sqlite3")).unwrap();
    let generation: i64 =
        db.query_row("SELECT next_token FROM settings", [], |row| row.get(0)).unwrap();
    let unrelated = tree.join(format!(".cowtree-install-{generation}"));
    fs::write(&unrelated, b"untracked data").unwrap();
    store.install(reader, version).unwrap();
    assert_eq!(fs::read(unrelated).unwrap(), b"untracked data");
    assert_eq!(fs::read(tree.join("file")).unwrap(), b"published");
}

#[test]
fn acknowledged_installations_never_roll_back_to_an_older_epoch() {
    let temp = tempfile::tempdir().unwrap();
    let (mut store, writer, reader, tree) = fixture(temp.path());
    let first = publish(&mut store, writer, 1, &[("file", Some(b"first"))]);
    store.install(reader, first).unwrap();
    let second = publish(&mut store, writer, 2, &[("file", Some(b"second"))]);
    store.install(reader, second).unwrap();
    assert!(store.install(reader, first).is_err());
    assert_eq!(fs::read(tree.join("file")).unwrap(), b"second");
    assert_eq!(store.binding(reader).unwrap().version, second);
    assert!(store.binding(reader).unwrap().pending.is_none());
}
