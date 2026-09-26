// Copyright (c) 2026 Windsor Nguyen

//! Deterministic schedules for construction-pin replacement during publication.

#![cfg(feature = "fault-injection")]
#![allow(clippy::unwrap_used)]

use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use cowtree_metadata::objects::{ObjectError, ObjectId, ObjectStore};
use cowtree_metadata::{LeafId, Limits, ResourcePath, Store};
use serde_json::{Value, json};

struct PausedProcess {
    /// Child is killed and reaped even when a regression assertion fails.
    child: Option<Child>,
}

impl PausedProcess {
    fn finish(mut self) -> Value {
        let output = self.child.take().unwrap().wait_with_output().unwrap();
        assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
        serde_json::from_slice(&output.stdout).unwrap()
    }
}

impl Drop for PausedProcess {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn paused_stage(
    root: &Path,
    leaf: LeafId,
    bytes: &[u8],
    point: &str,
    marker: &Path,
) -> PausedProcess {
    let child = Command::new(env!("CARGO_BIN_EXE_cowtree-metadata"))
        .arg(root)
        .env_remove("COWTREE_CRASH_AT")
        .env("COWTREE_PAUSE_AT", point)
        .env("COWTREE_PAUSE_FILE", marker)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut process = PausedProcess { child: Some(child) };
    let mut input = process.child.as_mut().unwrap().stdin.take().unwrap();
    serde_json::to_writer(&mut input, &json!({"op":"stage", "leaf":leaf, "data":bytes})).unwrap();
    writeln!(input).unwrap();
    drop(input);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !marker.try_exists().unwrap() {
        assert!(Instant::now() < deadline, "stage did not reach {point}");
        std::thread::sleep(Duration::from_millis(10));
    }
    process
}

#[test]
fn replaced_upload_pin_cannot_acknowledge_bytes_collected_from_an_older_stage() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let mut store = Store::create(&root, Limits::default()).unwrap();
    let leaf = store.create_leaf().unwrap();
    let bytes = b"same content, different upload lifetime";
    let object = ObjectId::from_bytes(bytes);
    let objects = ObjectStore::open(root.join("objects")).unwrap();
    let first_marker = directory.path().join("first-paused");
    let first = paused_stage(&root, leaf, bytes, "after-object-persist", &first_marker);
    assert_eq!(objects.read(&object).unwrap(), bytes);
    store.discard_upload(leaf, &object).unwrap();
    store.maintain().unwrap();
    assert!(matches!(objects.read(&object), Err(ObjectError::Missing { .. })));
    let second_marker = directory.path().join("second-paused");
    let second = paused_stage(&root, leaf, bytes, "after-upload-pin", &second_marker);
    std::fs::remove_file(&first_marker).unwrap();
    let stale_reply = first.finish();
    assert!(matches!(objects.read(&object), Err(ObjectError::Missing { .. })));
    assert_eq!(
        stale_reply["status"], "error",
        "stale stage acknowledged missing bytes: {stale_reply}"
    );
    assert!(stale_reply["message"].as_str().unwrap().contains("upload is not ready"));
    std::fs::remove_file(&second_marker).unwrap();
    assert_eq!(second.finish()["status"], "ok");
    assert_eq!(objects.read(&object).unwrap(), bytes);
}

#[test]
fn maintenance_reclaims_all_free_pages_after_large_reservation_teardown() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let mut store = Store::create(&root, Limits::default()).unwrap();
    let leaf = store.create_leaf().unwrap();
    let paths = (0..128)
        .map(|index| ResourcePath::parse(format!("{}-{index}", "x".repeat(2048))).unwrap())
        .collect();
    store.acquire(leaf, &paths).unwrap();
    store.drop_leaf(leaf).unwrap();
    let connection = rusqlite::Connection::open(root.join("metadata.sqlite3")).unwrap();
    let before: i64 =
        connection.pragma_query_value(None, "freelist_count", |row| row.get(0)).unwrap();
    assert!(before > 20, "fixture did not allocate enough pages: {before}");
    store.maintain().unwrap();
    let after: i64 =
        connection.pragma_query_value(None, "freelist_count", |row| row.get(0)).unwrap();
    assert_eq!(after, 0, "maintenance reclaimed only {} of {before} free pages", before - after);
}

#[test]
fn killed_writer_temporary_file_is_collected_even_while_its_hash_is_pinned() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let mut store = Store::create(&root, Limits::default()).unwrap();
    let leaf = store.create_leaf().unwrap();
    let bytes = b"pinned content survives";
    let object = store.stage(leaf, bytes).unwrap();
    let marker = directory.path().join("partial-write");
    let writer = paused_stage(&root, leaf, bytes, "after-object-temp-created", &marker);
    let pending = std::fs::read_dir(root.join("objects"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.file_name().unwrap().to_str().unwrap().starts_with(".pending-"))
        .unwrap();
    assert!(pending.exists());
    drop(writer);
    assert_eq!(store.maintain().unwrap().temporary_files_removed, 1);
    assert!(!pending.exists());
    assert_eq!(ObjectStore::open(root.join("objects")).unwrap().read(&object).unwrap(), bytes);
    assert_eq!(store.stage(leaf, bytes).unwrap(), object);
}

#[test]
fn temporary_collection_waits_for_active_writer_before_metadata_readiness() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let mut store = Store::create(&root, Limits::default()).unwrap();
    let leaf = store.create_leaf().unwrap();
    let marker = directory.path().join("active-write");
    let bytes = b"active writer survives collection";
    let writer = paused_stage(&root, leaf, bytes, "after-object-temp-created", &marker);
    let directory_fd = std::fs::File::open(root.join("objects")).unwrap();
    assert_eq!(
        rustix::fs::flock(&directory_fd, rustix::fs::FlockOperation::NonBlockingLockExclusive),
        Err(rustix::io::Errno::WOULDBLOCK),
    );
    let maintenance_root = root.clone();
    let maintenance =
        std::thread::spawn(move || Store::open(&maintenance_root).unwrap().maintain());
    std::fs::remove_file(&marker).unwrap();
    assert_eq!(writer.finish()["status"], "ok");
    assert_eq!(maintenance.join().unwrap().unwrap().temporary_files_removed, 0);
    let object = ObjectId::from_bytes(bytes);
    assert_eq!(ObjectStore::open(root.join("objects")).unwrap().read(&object).unwrap(), bytes);
}
