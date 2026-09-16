// Copyright (c) 2026 Windsor Nguyen

#![allow(clippy::unwrap_used)]

use cowtree_metadata::{
    Candidate, Entry, EntryKind, Error, Grant, Limits, ProposalInput, RequestId, ResourcePath,
    Store, Version,
};
use std::collections::BTreeSet;
use std::fs;

fn path(value: &str) -> ResourcePath {
    ResourcePath::parse(value).unwrap()
}
fn writer(store: &mut Store, name: &str) -> Grant {
    let leaf = store.create_leaf().unwrap();
    let grant = store.acquire(leaf, &BTreeSet::from([path(name)])).unwrap().remove(0);
    store.activate(&grant).unwrap()
}
fn publish(store: &mut Store, grant: &Grant, bytes: &[u8], sequence: u64) -> Candidate {
    let object = store.stage(grant.leaf, bytes).unwrap();
    store.edit(grant, Some(Entry { object, kind: EntryKind::File })).unwrap();
    let request = RequestId { leaf: grant.leaf, sequence };
    store.propose(ProposalInput { request, paths: BTreeSet::from([grant.path.clone()]) }).unwrap();
    let candidate = store.prepare(request).unwrap();
    store.commit(candidate.clone()).unwrap();
    candidate
}
#[cfg(feature = "fault-injection")]
fn crash(root: &std::path::Path, point: &str, input: serde_json::Value) {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new(env!("CARGO_BIN_EXE_cowtree-metadata"))
        .arg(root)
        .env("COWTREE_CRASH_AT", point)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    serde_json::to_writer(&mut stdin, &input).unwrap();
    writeln!(stdin).unwrap();
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(73), "{}", String::from_utf8_lossy(&output.stderr));
}

#[test]
#[cfg(feature = "fault-injection")]
fn unfinished_import_excludes_new_writers_until_the_source_is_recovered() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("store");
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("file"), b"source").unwrap();
    drop(Store::create(&root, Limits::default()).unwrap());
    crash(
        &root,
        "after-import-manifest",
        serde_json::json!({"op":"import_tree",
        "source":source,"paths":["file"]}),
    );
    let mut reopened = Store::open(&root).unwrap();
    assert!(reopened.create_leaf().is_err(), "unfinished import admitted a writer");
    reopened.maintain().unwrap();
    reopened.import_tree(&source, &[path("file")]).unwrap();
    assert_eq!(reopened.tip().unwrap().0.get(), 1);
    reopened.create_leaf().unwrap();
}

#[test]
#[cfg(feature = "fault-injection")]
fn pending_install_blocks_new_authority_but_preserves_committed_receipt_recovery() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("store");
    let tree = temp.path().join("tree");
    fs::create_dir(&tree).unwrap();
    let mut store = Store::create(&root, Limits::default()).unwrap();
    let grant = writer(&mut store, "file");
    store.bind_tree(grant.leaf, &tree, Version::new(0).unwrap()).unwrap();
    let candidate = publish(&mut store, &grant, b"committed", 1);
    let receipt = store.result(candidate.request).unwrap().unwrap();
    drop(store);
    crash(
        &root,
        "after-install-intent",
        serde_json::json!({"op":"install",
        "leaf":grant.leaf,"version":receipt.version}),
    );
    let mut store = Store::open(&root).unwrap();
    assert!(matches!(
        store.acquire(grant.leaf, &BTreeSet::from([path("other")])),
        Err(Error::NeedsRecovery(_))
    ));
    assert!(matches!(store.stage(grant.leaf, b"new edit"), Err(Error::NeedsRecovery(_))));
    assert!(matches!(store.edit(&grant, None), Err(Error::NeedsRecovery(_))));
    assert!(matches!(store.drop_leaf(grant.leaf), Err(Error::NeedsRecovery(_))));
    assert_eq!(store.commit(candidate).unwrap(), receipt);
    store.maintain().unwrap();
    store.recover(grant.leaf).unwrap();
    assert_eq!(fs::read(tree.join("file")).unwrap(), b"committed");
    assert_eq!(store.binding(grant.leaf).unwrap().pending, None);
}

#[test]
fn opening_version_one_preserves_pending_candidates_and_existing_receipts() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("store");
    let mut store = Store::create(&root, Limits::default()).unwrap();
    let grant = writer(&mut store, "file");
    let candidate = publish(&mut store, &grant, b"committed", 1);
    let receipt = store.result(candidate.request).unwrap().unwrap();
    let refreshed =
        store.acquire(grant.leaf, &BTreeSet::from([grant.path.clone()])).unwrap().remove(0);
    let object = store.stage(grant.leaf, b"pending").unwrap();
    store.edit(&refreshed, Some(Entry { object, kind: EntryKind::File })).unwrap();
    let request = RequestId { leaf: grant.leaf, sequence: 2 };
    store.propose(ProposalInput { request, paths: BTreeSet::from([grant.path.clone()]) }).unwrap();
    let pending = store.prepare(request).unwrap();
    drop(store);
    let connection = rusqlite::Connection::open(root.join("metadata.sqlite3")).unwrap();
    connection
        .execute_batch(
            "DROP TABLE bindings; DROP TABLE imports;
        ALTER TABLE proposals DROP COLUMN batch_id; ALTER TABLE views DROP COLUMN captured; PRAGMA user_version=1;",
        )
        .unwrap();
    drop(connection);
    let mut reopened = Store::open(&root).unwrap();
    assert_eq!(reopened.commit(candidate).unwrap(), receipt);
    assert_eq!(reopened.read(receipt.version, &grant.path).unwrap().unwrap(), b"committed");
    let latest = reopened.commit(pending).unwrap();
    assert_eq!(reopened.read(latest.version, &grant.path).unwrap().unwrap(), b"pending");
    reopened.maintain().unwrap();
    drop(reopened);
    assert_eq!(Store::open(&root).unwrap().tip().unwrap().0, latest.version);
}

#[test]
fn import_rejects_two_logical_paths_that_alias_one_case_insensitive_file() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("file"), b"one physical entry").unwrap();
    let aliases = source.join("FILE").exists();
    if !aliases {
        fs::write(source.join("FILE"), b"distinct entry").unwrap();
    }
    let mut store = Store::create(&temp.path().join("store"), Limits::default()).unwrap();
    let result = store.import_tree(&source, &[path("file"), path("FILE")]);
    if !aliases {
        let (version, _) = result.unwrap();
        assert_eq!(store.snapshot(version).unwrap().len(), 2);
        assert_eq!(store.read(version, &path("FILE")).unwrap().unwrap(), b"distinct entry");
        return;
    }
    assert!(result.is_err(), "case-distinct paths imported the same physical entry twice");
    assert_eq!(store.tip().unwrap().0.get(), 0);
}

#[test]
fn installation_does_not_acknowledge_case_aliases_as_distinct_entries() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("probe"), b"probe").unwrap();
    let aliases = temp.path().join("PROBE").exists();
    let root = temp.path().join("store");
    let tree = temp.path().join("tree");
    fs::create_dir(&tree).unwrap();
    let mut store = Store::create(&root, Limits::default()).unwrap();
    let leaf = store.create_leaf().unwrap();
    store.bind_tree(leaf, &tree, Version::new(0).unwrap()).unwrap();
    let paths = BTreeSet::from([path("file"), path("FILE")]);
    for grant in store.acquire(leaf, &paths).unwrap() {
        let grant = store.activate(&grant).unwrap();
        let object = store.stage(leaf, b"identical").unwrap();
        store.edit(&grant, Some(Entry { object, kind: EntryKind::File })).unwrap();
    }
    let request = RequestId { leaf, sequence: 1 };
    store.propose(ProposalInput { request, paths }).unwrap();
    if aliases {
        assert!(store.prepare(request).is_err());
        assert_eq!(store.tip().unwrap().0.get(), 0);
        assert_eq!(store.binding(leaf).unwrap().version.get(), 0);
        return;
    }
    let candidate = store.prepare(request).unwrap();
    let receipt = store.commit(candidate).unwrap();
    store.install(leaf, receipt.version).unwrap();
    fs::write(tree.join("file"), b"independent edit").unwrap();
    assert_eq!(fs::read(tree.join("FILE")).unwrap(), b"identical");
}

#[test]
fn concurrent_openers_serialize_version_one_migration() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("store");
    drop(Store::create(&root, Limits::default()).unwrap());
    let connection = rusqlite::Connection::open(root.join("metadata.sqlite3")).unwrap();
    connection
        .execute_batch(
            "DROP TABLE bindings; DROP TABLE imports;
        ALTER TABLE proposals DROP COLUMN batch_id; ALTER TABLE views DROP COLUMN captured; PRAGMA user_version=1;
        BEGIN IMMEDIATE;",
        )
        .unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let mut handles = Vec::new();
    for _ in 0..2 {
        let root = root.clone();
        let barrier = barrier.clone();
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            Store::open(&root).map(|store| store.tip().unwrap())
        }));
    }
    barrier.wait();
    std::thread::sleep(std::time::Duration::from_millis(300));
    connection.execute_batch("COMMIT").unwrap();
    for handle in handles {
        let result = handle.join().unwrap();
        assert!(result.is_ok(), "concurrent migration rejected an opener: {result:?}");
    }
}

#[test]
#[cfg(feature = "fault-injection")]
fn pending_install_retains_its_target_while_other_writers_advance_and_collect() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("store");
    let tree = temp.path().join("tree");
    fs::create_dir(&tree).unwrap();
    let limits = Limits { retained_epochs: 1, retained_receipts: 1, ..Limits::default() };
    let mut store = Store::create(&root, limits).unwrap();
    let reader = store.create_leaf().unwrap();
    store.bind_tree(reader, &tree, Version::new(0).unwrap()).unwrap();
    let mut grant = writer(&mut store, "file");
    publish(&mut store, &grant, b"one", 1);
    drop(store);
    crash(
        &root,
        "after-install-intent",
        serde_json::json!({"op":"install","leaf":reader,"version":1}),
    );
    let mut store = Store::open(&root).unwrap();
    for sequence in 2..=4 {
        grant = store.acquire(grant.leaf, &BTreeSet::from([grant.path.clone()])).unwrap().remove(0);
        publish(&mut store, &grant, format!("version {sequence}").as_bytes(), sequence);
        store.maintain().unwrap();
    }
    assert_eq!(store.snapshot(Version::new(1).unwrap()).unwrap().len(), 1);
    store.recover(reader).unwrap();
    assert_eq!(fs::read(tree.join("file")).unwrap(), b"one");
    store.install(reader, Version::new(4).unwrap()).unwrap();
    store.maintain().unwrap();
    assert!(matches!(store.snapshot(Version::new(1).unwrap()), Err(Error::SnapshotExpired(1))));
    assert_eq!(fs::read(tree.join("file")).unwrap(), b"version 4");
}
