// Copyright (c) 2026 Windsor Nguyen

#![allow(clippy::unwrap_used)]
use cowtree_metadata::{Entry, EntryKind, Limits, ProposalInput, RequestId, ResourcePath, Store};
use std::{collections::BTreeSet, fs};
fn path(s: &str) -> ResourcePath {
    ResourcePath::parse(s).unwrap()
}
#[test]
fn preserved_dirty_child_must_not_create_an_unrecoverable_install_plan() {
    let temp = tempfile::tempdir().unwrap();
    let tree = temp.path().join("tree");
    fs::create_dir_all(tree.join("node")).unwrap();
    fs::write(tree.join("node/child"), b"original").unwrap();
    let mut store = Store::create(&temp.path().join("store"), Limits::default()).unwrap();
    let (version, _) = store.import_tree(&tree, &[path("node/child")]).unwrap();
    let leaf = store.create_leaf().unwrap();
    store.bind_tree(leaf, &tree, version).unwrap();
    let paths = BTreeSet::from([path("node"), path("node/child")]);
    let grants = store.acquire(leaf, &paths).unwrap();
    let grants: Vec<_> = grants.iter().map(|g| store.activate(g).unwrap()).collect();
    let child = grants.iter().find(|g| g.path == path("node/child")).unwrap();
    let parent = grants.iter().find(|g| g.path == path("node")).unwrap();
    store.edit(child, None).unwrap();
    let object = store.stage(leaf, b"replacement parent").unwrap();
    store.edit(parent, Some(Entry { object, kind: EntryKind::File })).unwrap();
    let request = RequestId { leaf, sequence: 1 };
    store.propose(ProposalInput { request, paths }).unwrap();
    let candidate = store.prepare(request).unwrap();
    fs::write(tree.join("node/child"), b"later edit").unwrap();
    store.capture_files(std::slice::from_ref(child)).unwrap();
    let receipt = store.commit(candidate).unwrap();
    assert!(store.install(leaf, receipt.version).is_err());
    assert_eq!(fs::read(tree.join("node/child")).unwrap(), b"later edit");
    assert!(
        store.binding(leaf).unwrap().pending.is_none(),
        "known namespace conflict was durably recorded and cannot recover"
    );
}

#[test]
fn releasing_authority_does_not_erase_proof_of_a_published_local_file() {
    let temp = tempfile::tempdir().unwrap();
    let tree = temp.path().join("tree");
    fs::create_dir(&tree).unwrap();
    let mut store = Store::create(&temp.path().join("store"), Limits::default()).unwrap();
    let leaf = store.create_leaf().unwrap();
    store.bind_tree(leaf, &tree, cowtree_metadata::Version::new(0).unwrap()).unwrap();
    let paths = BTreeSet::from([path("new")]);
    let grant = store.acquire(leaf, &paths).unwrap().remove(0);
    let grant = store.activate(&grant).unwrap();
    fs::write(tree.join("new"), b"published local edit").unwrap();
    store.capture_files(&[grant]).unwrap();
    let request = RequestId { leaf, sequence: 1 };
    store.propose(ProposalInput { request, paths: paths.clone() }).unwrap();
    let candidate = store.prepare(request).unwrap();
    let receipt = store.commit(candidate).unwrap();
    let grant = store.acquire(leaf, &paths).unwrap().remove(0);
    store.release(&grant).unwrap();
    store.install(leaf, receipt.version).unwrap();
    assert_eq!(fs::read(tree.join("new")).unwrap(), b"published local edit");
}
