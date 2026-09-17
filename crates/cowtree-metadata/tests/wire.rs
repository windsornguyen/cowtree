// Copyright (c) 2026 Windsor Nguyen

//! Machine-readable CLI failures preserve protocol context and request framing.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use cowtree_metadata::{
    Candidate, Entry, EntryKind, Grant, Limits, ProposalInput, RequestId, ResourcePath, Store,
};
use serde_json::{Value, json};

fn cli(root: &Path, request: Value) -> Value {
    let mut child = Command::new(env!("CARGO_BIN_EXE_cowtree-metadata"))
        .arg(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    serde_json::to_writer(&mut input, &request).unwrap();
    writeln!(input).unwrap();
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    serde_json::from_slice(&output.stdout).unwrap()
}

fn writer(store: &mut Store, path: &str) -> Grant {
    let leaf = store.create_leaf().unwrap();
    let paths = BTreeSet::from([ResourcePath::parse(path).unwrap()]);
    let grant = store.acquire(leaf, &paths).unwrap().remove(0);
    store.activate(&grant).unwrap()
}

fn proposal(store: &mut Store, grant: &Grant) -> Candidate {
    let object = store.stage(grant.leaf, b"published bytes").unwrap();
    store.edit(grant, Some(Entry { object, kind: EntryKind::File })).unwrap();
    let request = RequestId { leaf: grant.leaf, sequence: 1 };
    store.propose(ProposalInput { request, paths: BTreeSet::from([grant.path.clone()]) }).unwrap();
    store.prepare(request).unwrap()
}

fn failure(response: &Value, code: &str, retry: &str) {
    assert_eq!(response["status"], "error");
    assert_eq!(response["code"], code);
    assert_eq!(response["retry_action"], retry);
    assert!(response["message"].is_string());
    assert!(response["details"]["kind"].is_string());
}

#[test]
fn tip_changes_stale_tokens_and_expired_requests_have_distinct_codes() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let limits = Limits { retained_receipts: 1, ..Limits::default() };
    let mut store = Store::create(&root, limits).unwrap();
    let left = writer(&mut store, "left");
    let right = writer(&mut store, "right");
    let first = proposal(&mut store, &left);
    let second = proposal(&mut store, &right);
    store.commit(first.clone()).unwrap();
    let response = cli(&root, json!({"op":"commit","candidate":second}));
    failure(&response, "tip_changed", "reprepare");
    assert_eq!(response["details"], json!({"kind":"tip","expected":0,"actual":1}));
    let second = store.prepare(second.request).unwrap();
    store.commit(second).unwrap();
    store.maintain().unwrap();
    let response = cli(&root, json!({"op":"result","request":first.request}));
    failure(&response, "request_expired", "none");
    store.revoke(&left.path).unwrap();
    let response = cli(&root, json!({"op":"activate","grant":left}));
    failure(&response, "stale_token", "none");
    assert_eq!(response["details"]["path"], "left");
}

#[test]
fn corrupt_objects_preserve_expected_and_observed_digests() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let mut store = Store::create(&root, Limits::default()).unwrap();
    let grant = writer(&mut store, "file");
    let candidate = proposal(&mut store, &grant);
    let receipt = store.commit(candidate).unwrap();
    let entry = store.snapshot(receipt.version).unwrap().remove(&grant.path).unwrap();
    std::fs::write(root.join("objects").join(entry.object.as_str()), b"corrupted").unwrap();
    let response = cli(&root, json!({"op":"read","version":receipt.version,"path":"file"}));
    failure(&response, "object_corrupt", "none");
    assert_eq!(response["details"]["expected"], entry.object.as_str());
    assert_ne!(response["details"]["expected"], response["details"]["actual"]);
}

#[test]
fn busy_database_is_retryable_but_input_capacity_is_not() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let limits = Limits { max_leaves: 1, ..Limits::default() };
    let mut store = Store::create(&root, limits).unwrap();
    store.create_leaf().unwrap();
    let response = cli(&root, json!({"op":"create_leaf"}));
    failure(&response, "limit_exceeded", "none");
    assert_eq!(response["details"]["resource"], "active_leaves");
    let connection = rusqlite::Connection::open(root.join("metadata.sqlite3")).unwrap();
    connection.execute_batch("BEGIN IMMEDIATE").unwrap();
    let response = cli(&root, json!({"op":"create_leaf"}));
    failure(&response, "database_busy", "retry_same_request");
    assert_eq!(response["details"]["extended_code"], 5);
    connection.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn malformed_and_oversized_requests_do_not_consume_the_following_request() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let mut child = Command::new(env!("CARGO_BIN_EXE_cowtree-metadata"))
        .arg(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    writeln!(input, "{{invalid").unwrap();
    let chunk = vec![b' '; 1024 * 1024];
    for _ in 0..129 {
        input.write_all(&chunk).unwrap();
    }
    writeln!(input).unwrap();
    writeln!(input, "{{\"op\":\"init\"}}").unwrap();
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let responses: Vec<Value> = serde_json::Deserializer::from_slice(&output.stdout)
        .into_iter()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(responses.len(), 3);
    failure(&responses[0], "invalid_request", "none");
    failure(&responses[1], "invalid_request", "none");
    assert_eq!(responses[1]["details"]["kind"], "request_too_large");
    assert_eq!(responses[2]["status"], "ok");
}

#[test]
#[cfg(not(feature = "fault-injection"))]
fn production_binary_ignores_fault_injection_environment() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let mut store = Store::create(&root, Limits::default()).unwrap();
    let leaf = store.create_leaf().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_cowtree-metadata"))
        .arg(&root)
        .env("COWTREE_CRASH_AT", "after-object-persist")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    writeln!(input, "{}", json!({"op":"stage","leaf":leaf,"data":[1,2,3]})).unwrap();
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["status"], "ok");
}

#[test]
fn batch_commands_publish_one_version_and_return_all_receipts() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let mut store = Store::create(&root, Limits::default()).unwrap();
    let left = writer(&mut store, "left");
    let right = writer(&mut store, "right");
    let first = proposal(&mut store, &left);
    let second = proposal(&mut store, &right);
    let prepared = cli(
        &root,
        json!({"op":"prepare_batch","expected_tip":0,
        "requests":[first.request,second.request]}),
    );
    assert_eq!(prepared["status"], "ok");
    assert_eq!(prepared["output"]["kind"], "batch_candidate");
    let committed = cli(
        &root,
        json!({"op":"commit_batch",
        "candidate":prepared["output"]["value"]}),
    );
    assert_eq!(committed["status"], "ok");
    assert_eq!(committed["output"]["kind"], "batch_receipt");
    let receipts = committed["output"]["value"]["receipts"].as_array().unwrap();
    assert_eq!(receipts.len(), 2);
    assert_eq!(receipts[0]["version"], 1);
    assert_eq!(receipts[1]["version"], 1);
    assert_eq!(receipts[0]["root"], receipts[1]["root"]);
}

#[test]
fn wal_capacity_requires_maintenance_instead_of_blind_retry() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let limits = Limits { max_wal_bytes: 4096, ..Limits::default() };
    let store = Store::create(&root, limits).unwrap();
    let response = cli(&root, json!({"op":"create_leaf"}));
    failure(&response, "limit_exceeded", "run_maintenance");
    assert_eq!(response["details"]["resource"], "wal_bytes");
    drop(store);
}

#[test]
fn sqlite_constraint_errors_are_not_classified_as_retryable() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let mut store = Store::create(&root, Limits::default()).unwrap();
    store.create_leaf().unwrap();
    let connection = rusqlite::Connection::open(root.join("metadata.sqlite3")).unwrap();
    connection.execute("UPDATE settings SET next_leaf=1", []).unwrap();
    let response = cli(&root, json!({"op":"create_leaf"}));
    failure(&response, "sqlite", "none");
    assert_eq!(response["details"]["extended_code"], 1555);
}
