// Copyright (c) 2026 Windsor Nguyen

//! SIGKILL cuts around each import transaction preserve its durable cursor and tip.
#![cfg(feature = "fault-injection")]
#![allow(clippy::unwrap_used)]

use cowtree_metadata::objects::ObjectId;
use cowtree_metadata::{Entry, EntryKind, Limits, ResourcePath, Snapshot, Store, Version};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

fn fixture(directory: &Path) -> Snapshot {
    fs::create_dir(directory).unwrap();
    let mut manifest = BTreeMap::new();
    for index in 0..4 {
        let name = format!("file-{index}");
        let bytes = format!("complete payload {index}");
        fs::write(directory.join(&name), &bytes).unwrap();
        manifest.insert(
            ResourcePath::parse(name).unwrap(),
            Entry { object: ObjectId::from_bytes(bytes.as_bytes()), kind: EntryKind::File },
        );
    }
    manifest
}

fn pause(root: &Path, marker: &Path, point: &str, request: &Value) -> Child {
    let mut child = Command::new(env!("CARGO_BIN_EXE_cowtree-metadata"))
        .arg(root)
        .env("COWTREE_PAUSE_AT", point)
        .env("COWTREE_PAUSE_FILE", marker)
        .env_remove("COWTREE_CRASH_AT")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    serde_json::to_writer(&mut input, request).unwrap();
    writeln!(input).unwrap();
    drop(input);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !marker.exists() {
        assert!(child.try_wait().unwrap().is_none(), "child exited before pause: {point}");
        assert!(Instant::now() < deadline, "child missed pause: {point}");
        thread::sleep(Duration::from_millis(10));
    }
    child
}

#[test]
fn killed_begin_chunk_and_finish_recover_without_partial_publication() {
    for point in [
        "before-import-begin-commit",
        "after-import-begin-commit",
        "before-object-group-flush",
        "after-object-group-flush",
        "before-import-chunk-commit",
        "after-import-chunk-commit",
        "before-import-finish-commit",
        "after-import-finish-commit",
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("source");
        let manifest = fixture(&source);
        let root = temporary.path().join("store");
        let mut store = Store::create(&root, Limits::default()).unwrap();
        let request = if point.contains("begin") {
            json!({"op":"begin_import", "manifest":manifest})
        } else {
            let progress = store.begin_import(manifest.clone()).unwrap();
            if point.contains("finish") {
                store.import_chunk(&progress.root, &source).unwrap();
                json!({"op":"finish_import", "root":progress.root})
            } else {
                json!({"op":"import_chunk", "root":progress.root, "source":source})
            }
        };
        drop(store);
        let mut child = pause(&root, &temporary.path().join("paused"), point, &request);
        child.kill().unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty(), "uncommitted reply at {point}");
        let mut store = Store::open(&root).unwrap();
        let expected_version = u64::from(point == "after-import-finish-commit");
        assert_eq!(store.tip().unwrap().0.get(), expected_version, "{point}");
        let before = store.status_import().unwrap();
        if point == "before-import-begin-commit" {
            assert!(before.is_none());
        } else if point == "before-import-chunk-commit" || point.contains("object-group-flush") {
            assert_eq!(before.unwrap().completed, 0);
        } else if point == "after-import-chunk-commit" {
            assert_eq!(before.unwrap().completed, manifest.len());
        }
        store.maintain().unwrap();
        let progress = store.begin_import(manifest.clone()).unwrap();
        if !progress.complete {
            store.import_chunk(&progress.root, &source).unwrap();
        }
        let finished = store.finish_import(&progress.root).unwrap();
        assert!(finished.complete);
        assert_eq!(store.tip().unwrap().0.get(), 1);
        assert_eq!(store.snapshot(Version::new(1).unwrap()).unwrap(), manifest);
        for path in manifest.keys() {
            assert_eq!(
                store.read(Version::new(1).unwrap(), path).unwrap().unwrap(),
                fs::read(source.join(path.as_str())).unwrap()
            );
        }
    }
}

#[test]
fn collection_waits_for_an_import_chunk_and_keeps_its_objects() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("source");
    let manifest = fixture(&source);
    let root = temporary.path().join("store");
    let mut store = Store::create(&root, Limits::default()).unwrap();
    let progress = store.begin_import(manifest).unwrap();
    let mut collector = Store::open(&root).unwrap();
    drop(store);
    let marker = temporary.path().join("paused");
    let child = pause(
        &root,
        &marker,
        "before-import-chunk-commit",
        &json!({"op":"import_chunk", "root":progress.root, "source":source}),
    );
    let (sender, receiver) = std::sync::mpsc::channel();
    let collection = thread::spawn(move || {
        sender.send(collector.maintain()).unwrap();
    });
    assert!(receiver.recv_timeout(Duration::from_millis(100)).is_err());
    fs::remove_file(marker).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert_eq!(receiver.recv_timeout(Duration::from_secs(5)).unwrap().unwrap().objects_removed, 0);
    collection.join().unwrap();
    let mut store = Store::open(&root).unwrap();
    assert_eq!(store.status_import().unwrap().unwrap().completed, 4);
    store.finish_import(&progress.root).unwrap();
}

#[test]
fn a_failed_group_flush_cannot_acknowledge_chunk_progress() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("source");
    let manifest = fixture(&source);
    let root = temporary.path().join("store");
    let mut store = Store::create(&root, Limits::default()).unwrap();
    let progress = store.begin_import(manifest).unwrap();
    drop(store);
    let mut child = Command::new(env!("CARGO_BIN_EXE_cowtree-metadata"))
        .arg(&root)
        .env("COWTREE_IO_ERROR_AT", "object-group-flush")
        .env_remove("COWTREE_CRASH_AT")
        .env_remove("COWTREE_PAUSE_AT")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    serde_json::to_writer(
        &mut input,
        &json!({"op":"import_chunk", "root":progress.root, "source":source}),
    )
    .unwrap();
    writeln!(input).unwrap();
    drop(input);
    let output = child.wait_with_output().unwrap();
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["status"], "error");
    assert_eq!(response["code"], "object_io");
    let mut store = Store::open(&root).unwrap();
    assert_eq!(store.tip().unwrap().0.get(), 0);
    assert_eq!(store.status_import().unwrap().unwrap().completed, 0);
    store.maintain().unwrap();
    store.import_chunk(&progress.root, &source).unwrap();
    store.finish_import(&progress.root).unwrap();
    assert_eq!(store.tip().unwrap().0.get(), 1);
}
