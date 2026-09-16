// Copyright (c) 2026 Windsor Nguyen

//! Real process exits at publication boundaries preserve pins and acknowledgement recovery.

#![cfg(feature = "fault-injection")]
#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};

use cowtree_metadata::objects::{ObjectId, ObjectStore};
use cowtree_metadata::{
    Candidate, Entry, EntryKind, Error, Grant, Limits, ProposalInput, Receipt, RequestId,
    ResourcePath, Store, Version,
};
use serde_json::{Value, json};

fn fixture() -> (tempfile::TempDir, Store, Grant) {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::create(&directory.path().join("store"), Limits::default()).unwrap();
    let grant = writer(&mut store, "file");
    (directory, store, grant)
}

fn writer(store: &mut Store, path: &str) -> Grant {
    let leaf = store.create_leaf().unwrap();
    let grant = store
        .acquire(leaf, &BTreeSet::from([ResourcePath::parse(path).unwrap()]))
        .unwrap()
        .remove(0);
    store.activate(&grant).unwrap()
}

fn propose(store: &mut Store, grant: &Grant, bytes: &[u8]) -> RequestId {
    let object = store.stage(grant.leaf, bytes).unwrap();
    store.edit(grant, Some(Entry { object, kind: EntryKind::File })).unwrap();
    let request = RequestId { leaf: grant.leaf, sequence: 1 };
    store.propose(ProposalInput { request, paths: BTreeSet::from([grant.path.clone()]) }).unwrap();
    request
}

fn child(root: &Path, fault: Option<&str>) -> Child {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cowtree-metadata"));
    command.arg(root).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    command.env_remove("COWTREE_CRASH_AT");
    if let Some(point) = fault {
        command.env("COWTREE_CRASH_AT", point);
    }
    command.spawn().unwrap()
}

fn send(child: &mut Child, request: &Value) {
    let mut input = child.stdin.take().unwrap();
    serde_json::to_writer(&mut input, request).unwrap();
    writeln!(input).unwrap();
}

fn crash(root: &Path, point: &str, request: Value) {
    let mut process = child(root, Some(point));
    send(&mut process, &request);
    let output = process.wait_with_output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(73),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty(), "crash must precede acknowledgement");
}

fn response(output: Output) -> Value {
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn exit_before_sql_commit_rolls_back_tip_and_allows_exact_candidate_retry() {
    let (directory, mut store, grant) = fixture();
    let root = directory.path().join("store");
    let request = propose(&mut store, &grant, b"before-commit bytes");
    let candidate = store.prepare(request).unwrap();
    drop(store);
    crash(&root, "before-sql-commit", json!({"op":"commit", "candidate":candidate}));
    let mut reopened = Store::open(&root).unwrap();
    assert_eq!(reopened.tip().unwrap().0.get(), 0);
    assert_eq!(reopened.result(request).unwrap(), None);
    assert_eq!(reopened.read(Version::new(0).unwrap(), &grant.path).unwrap(), None);
    reopened.maintain().unwrap();
    let receipt = reopened.commit(candidate.clone()).unwrap();
    assert_eq!(receipt.version.get(), 1);
    assert_eq!(reopened.commit(candidate).unwrap(), receipt);
    assert_eq!(
        reopened.read(receipt.version, &grant.path).unwrap(),
        Some(b"before-commit bytes".to_vec())
    );
}

#[test]
fn exit_after_sql_commit_recovers_receipt_and_retry_publishes_exactly_once() {
    let (directory, mut store, grant) = fixture();
    let root = directory.path().join("store");
    let request = propose(&mut store, &grant, b"acknowledgement lost");
    let candidate = store.prepare(request).unwrap();
    drop(store);
    crash(&root, "after-sql-commit", json!({"op":"commit", "candidate":candidate}));
    let mut reopened = Store::open(&root).unwrap();
    let receipt = reopened.result(request).unwrap().unwrap();
    assert_eq!(receipt.version.get(), 1);
    assert_eq!(
        reopened.read(receipt.version, &grant.path).unwrap(),
        Some(b"acknowledgement lost".to_vec())
    );
    assert_eq!(reopened.commit(candidate.clone()).unwrap(), receipt);
    drop(reopened);
    let mut retry = child(&root, None);
    send(&mut retry, &json!({"op":"commit", "candidate":candidate}));
    let reply = response(retry.wait_with_output().unwrap());
    assert_eq!(reply["status"], "ok");
    assert_eq!(
        serde_json::from_value::<Receipt>(reply["output"]["value"].clone()).unwrap(),
        receipt
    );
    assert_eq!(Store::open(&root).unwrap().tip().unwrap().0, receipt.version);
    let connection = rusqlite::Connection::open(root.join("metadata.sqlite3")).unwrap();
    let epochs: i64 =
        connection.query_row("SELECT count(*) FROM epochs", [], |row| row.get(0)).unwrap();
    assert_eq!(epochs, 2);
}

#[test]
fn persisted_candidate_requires_readiness_before_publication() {
    let (directory, mut store, grant) = fixture();
    let root = directory.path().join("store");
    let request = propose(&mut store, &grant, b"candidate survives");
    drop(store);
    crash(&root, "after-candidate-persist", json!({"op":"prepare", "request":request}));
    let connection = rusqlite::Connection::open(root.join("metadata.sqlite3")).unwrap();
    let (attempt, parent, digest, ready): (i64, i64, String, bool) = connection
        .query_row(
            "SELECT attempt,parent,root,ready FROM proposals WHERE leaf=?1 AND sequence=?2",
            rusqlite::params![
                i64::try_from(request.leaf.get()).unwrap(),
                i64::try_from(request.sequence).unwrap()
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert!(!ready);
    let guessed = Candidate {
        request,
        attempt: u64::try_from(attempt).unwrap(),
        parent: Version::new(u64::try_from(parent).unwrap()).unwrap(),
        root: ObjectId::parse(&digest).unwrap(),
    };
    let mut reopened = Store::open(&root).unwrap();
    assert!(matches!(reopened.commit(guessed.clone()), Err(Error::CandidateNotReady)));
    reopened.maintain().unwrap();
    let objects = ObjectStore::open(root.join("objects")).unwrap();
    assert!(!objects.read(&guessed.root).unwrap().is_empty());
    let prepared = reopened.prepare(request).unwrap();
    assert!(prepared.attempt > guessed.attempt);
    assert!(matches!(reopened.commit(guessed), Err(Error::CandidateMismatch)));
    let receipt = reopened.commit(prepared).unwrap();
    assert_eq!(
        reopened.read(receipt.version, &grant.path).unwrap(),
        Some(b"candidate survives".to_vec())
    );
}

#[test]
fn interrupted_upload_pin_survives_collection_until_stage_retry() {
    let (directory, store, grant) = fixture();
    let root = directory.path().join("store");
    let bytes = b"upload survives process exit";
    let expected = ObjectId::from_bytes(bytes);
    drop(store);
    crash(
        &root,
        "after-object-persist",
        json!({"op":"stage", "leaf":grant.leaf, "data":bytes.to_vec()}),
    );
    let mut reopened = Store::open(&root).unwrap();
    let value = Entry { object: expected.clone(), kind: EntryKind::File };
    assert!(matches!(reopened.edit(&grant, Some(value.clone())), Err(Error::UploadNotReady)));
    reopened.maintain().unwrap();
    let objects = ObjectStore::open(root.join("objects")).unwrap();
    assert_eq!(objects.read(&expected).unwrap(), bytes);
    assert_eq!(reopened.stage(grant.leaf, bytes).unwrap(), expected);
    reopened.edit(&grant, Some(value)).unwrap();
    assert_eq!(reopened.view(grant.leaf).unwrap()[0].value.as_ref().unwrap().object, expected);
}

#[test]
fn independent_commit_processes_have_one_winner_then_rebase_disjoint_work() {
    let (directory, mut store, left) = fixture();
    let root = directory.path().join("store");
    let right = writer(&mut store, "other");
    let left_request = propose(&mut store, &left, b"left bytes");
    let right_request = propose(&mut store, &right, b"right bytes");
    let left_candidate = store.prepare(left_request).unwrap();
    let right_candidate = store.prepare(right_request).unwrap();
    assert_eq!(left_candidate.parent, right_candidate.parent);
    drop(store);
    let mut first = child(&root, None);
    let mut second = child(&root, None);
    send(&mut first, &json!({"op":"commit", "candidate":left_candidate}));
    send(&mut second, &json!({"op":"commit", "candidate":right_candidate}));
    let first_response = response(first.wait_with_output().unwrap());
    let second_response = response(second.wait_with_output().unwrap());
    let replies = [&first_response, &second_response];
    assert_eq!(replies.iter().filter(|reply| reply["status"] == "ok").count(), 1);
    let failed = replies.iter().find(|reply| reply["status"] == "error").unwrap();
    assert!(failed["message"].as_str().unwrap().contains("workspace tip changed"));
    let retry = if first_response["status"] == "error" { left_request } else { right_request };
    let mut reopened = Store::open(&root).unwrap();
    assert_eq!(reopened.tip().unwrap().0.get(), 1);
    let fresh = reopened.prepare(retry).unwrap();
    let receipt = reopened.commit(fresh).unwrap();
    assert_eq!(receipt.version.get(), 2);
    assert_eq!(reopened.read(receipt.version, &left.path).unwrap(), Some(b"left bytes".to_vec()));
    assert_eq!(reopened.read(receipt.version, &right.path).unwrap(), Some(b"right bytes".to_vec()));
}
