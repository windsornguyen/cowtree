// Copyright (c) 2026 Windsor Nguyen

//! Recover durable imports, batch acknowledgements, and quarantine without changing intent.

#![cfg(all(unix, feature = "fault-injection"))]
mod support;

use cowtree_metadata::{BatchCandidate, LeafId};
use cowtree_workspace::{Config, Leaf, Node, Validation};
use std::{fs, process::Command};
use support::{Fixture, TestResult, decode, text};

#[test]
fn checkpoint_recovery_verifies_bytes_before_publishing_its_ref() -> TestResult {
    #[derive(serde::Deserialize)]
    struct Intent {
        node: cowtree_workspace::NodeId,
    }
    for tamper in [false, true] {
        let Some(fixture) = Fixture::new()? else {
            return Ok(());
        };
        let leaf = fixture.fork("first")?;
        fs::write(leaf.path.join("file"), b"private checkpoint")?;
        let output = fixture
            .command(&["seal", &leaf.id.get().to_string()])
            .env("COWTREE_WORKSPACE_CRASH_AT", "after-node-record")
            .output()?;
        assert_eq!(output.status.code(), Some(73));
        let operation = fs::read_dir(fixture.store.join("operations"))?
            .next()
            .transpose()?
            .ok_or("seal intent missing")?
            .path();
        let intent: Intent = serde_json::from_slice(&fs::read(operation.join("seal.json"))?)?;
        let directory = fixture.store.join("nodes").join(intent.node.as_str());
        let node: Node = serde_json::from_slice(&fs::read(directory.join("node.json"))?)?;
        let reference = format!("refs/cowtree/nodes/{}", node.id.as_str());
        let reference_status = || {
            Command::new("git")
                .arg("-C")
                .arg(&fixture.source)
                .args(["show-ref", "--verify", "--quiet", &reference])
                .status()
        };
        assert_eq!(reference_status()?.code(), Some(1));
        if tamper {
            fs::write(directory.join("tree/file"), b"changed after crash")?;
        }
        let recovered = fixture.command(&["recover"]).output()?;
        assert_eq!(recovered.status.success(), !tamper);
        let actual: Leaf = serde_json::from_slice(&fs::read(
            fixture.store.join("leaves").join(format!("{}.json", leaf.id.get())),
        )?)?;
        if tamper {
            assert_eq!(actual.node, leaf.node);
            assert_eq!(reference_status()?.code(), Some(1));
        } else {
            assert_eq!(actual.node, node.id);
            assert!(reference_status()?.success());
            assert!(!operation.exists());
        }
        assert_eq!(fs::read(leaf.path.join("file"))?, b"private checkpoint");
    }
    Ok(())
}

#[test]
fn interrupted_import_resumes_the_frozen_snapshot() -> TestResult {
    let Some(fixture) = Fixture::new()? else {
        return Ok(());
    };
    let store = fixture.directory.path().join("interrupted-import");
    let binary = env!("CARGO_BIN_EXE_cowtree");
    let output = Command::new(binary)
        .args(["workspace", "--root"])
        .arg(&store)
        .args(["init", "--source"])
        .arg(&fixture.source)
        .env("COWTREE_CRASH_AT", "after-import-chunk-commit")
        .output()?;
    assert_eq!(output.status.code(), Some(73));
    assert!(!store.exists());
    fs::write(fixture.source.join("file"), b"changed after capture")?;
    let recovered: bool = decode(
        Command::new(binary).args(["workspace", "--root"]).arg(&store).arg("recover").output()?,
    )?;
    assert!(recovered);
    let target = fixture.directory.path().join("recovered-leaf");
    let leaf: Leaf = decode(
        Command::new(binary)
            .args(["workspace", "--root"])
            .arg(&store)
            .arg("fork")
            .arg(&target)
            .output()?,
    )?;
    assert_eq!(fs::read(leaf.path.join("file"))?, b"original\n");
    assert_eq!(fs::read(fixture.source.join("file"))?, b"changed after capture");
    Ok(())
}

#[test]
fn lost_batch_acknowledgement_recovers_every_member() -> TestResult {
    let Some(fixture) = Fixture::new()? else {
        return Ok(());
    };
    let first = fixture.fork("first")?;
    let second = fixture.fork("second")?;
    fixture.capture(&first, "file", b"first\n")?;
    fixture.capture(&second, "other", b"second\n")?;
    let candidate: BatchCandidate = fixture.call(&[
        "prepare-batch",
        &first.id.get().to_string(),
        &second.id.get().to_string(),
    ])?;
    let path = fixture.directory.path().join("candidate.json");
    fs::write(&path, serde_json::to_vec(&candidate)?)?;
    let _: Validation =
        fixture.call(&["check-batch", "--candidate", text(&path)?, "--", "/usr/bin/true"])?;
    fs::write(first.path.join("file"), b"later first\n")?;
    let output = fixture
        .command(&["commit-batch", "--candidate", text(&path)?])
        .env("COWTREE_WORKSPACE_CRASH_AT", "after-batch-commit")
        .output()?;
    assert_eq!(output.status.code(), Some(73));
    let _: Vec<LeafId> = fixture.call(&["recover"])?;
    let leaves: Vec<Leaf> = fixture.call(&["list"])?;
    for id in [first.id, second.id] {
        let leaf = leaves.iter().find(|leaf| leaf.id == id).ok_or("batch member missing")?;
        assert!(leaf.pending.is_none());
        assert!(leaf.last_receipt.is_some());
    }
    assert_eq!(fs::read(first.path.join("file"))?, b"later first\n");
    let published = fixture.fork("published")?;
    assert_eq!(fs::read(published.path.join("file"))?, b"first\n");
    assert_eq!(fs::read(published.path.join("other"))?, b"second\n");
    Ok(())
}

#[test]
fn quarantine_recovery_preserves_live_checkpoints() -> TestResult {
    let Some(fixture) = Fixture::new()? else {
        return Ok(());
    };
    let leaf = fixture.fork("first")?;
    let retired: Node = fixture.call(&["seal", &leaf.id.get().to_string()])?;
    let _: () = fixture.call(&["release", retired.id.as_str()])?;
    fs::write(leaf.path.join("file"), b"private checkpoint")?;
    let live: Node = fixture.call(&["seal", &leaf.id.get().to_string()])?;
    let output = fixture
        .command(&["collect"])
        .env("COWTREE_WORKSPACE_CRASH_AT", "after-quarantine-intent")
        .output()?;
    assert_eq!(output.status.code(), Some(73));
    let _: Vec<LeafId> = fixture.call(&["recover"])?;
    assert!(!fixture.store.join("nodes").join(retired.id.as_str()).exists());
    assert!(fixture.store.join("nodes").join(live.id.as_str()).exists());
    assert_eq!(fs::read(leaf.path.join("file"))?, b"private checkpoint");
    assert!(fs::read_dir(fixture.store.join("trash"))?.next().is_none());
    Ok(())
}

#[test]
fn stored_executable_provenance_is_not_a_runtime_dependency() -> TestResult {
    let Some(fixture) = Fixture::new()? else {
        return Ok(());
    };
    let path = fixture.store.join("workspace.json");
    let mut config: Config = serde_json::from_slice(&fs::read(&path)?)?;
    config.binary = fixture.directory.path().join("removed-old-runtime");
    fs::write(path, serde_json::to_vec(&config)?)?;
    let leaf = fixture.fork("first")?;
    fixture.capture(&leaf, "file", b"native runtime")?;
    let _: cowtree_metadata::Candidate = fixture.call(&["prepare", &leaf.id.get().to_string()])?;
    fixture.check(&leaf, "test \"$(cat file)\" = 'native runtime'")?;
    let _: cowtree_metadata::Receipt = fixture.call(&["commit", &leaf.id.get().to_string()])?;
    Ok(())
}
