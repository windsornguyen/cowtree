// Copyright (c) 2026 Windsor Nguyen

//! Exercise the shipped native CLI across publication, cache inheritance, and recovery.

#![cfg(unix)]

mod support;
use cowtree_metadata::{BatchCandidate, Receipt};
use cowtree_workspace::{Leaf, Node, Validation};
use std::{fs, process::Command};
use support::{Fixture, TestResult, decode, text};

#[test]
fn publication_preserves_later_edits_and_inherits_verified_cache() -> TestResult {
    let Some(fixture) = Fixture::new()? else {
        return Ok(());
    };
    let first = fixture.fork("first")?;
    let second = fixture.fork("second")?;
    fixture.capture(&first, "file", b"captured\n")?;
    let _: cowtree_metadata::Candidate = fixture.call(&["prepare", &first.id.get().to_string()])?;
    fs::write(first.path.join("file"), b"later private\n")?;
    fixture.check(&first, "test \"$(cat file)\" = captured && printf built > target/cache")?;
    let receipt: Receipt = fixture.call(&["commit", &first.id.get().to_string()])?;
    let repeated: Option<Receipt> = fixture.call(&[
        "result",
        &first.id.get().to_string(),
        &receipt.request.sequence.to_string(),
    ])?;
    assert_eq!(repeated, Some(receipt));
    assert_eq!(fs::read(first.path.join("file"))?, b"later private\n");
    assert_eq!(fs::read(second.path.join("file"))?, b"original\n");
    let _: Leaf = fixture.call(&["sync", &second.id.get().to_string()])?;
    assert_eq!(fs::read(second.path.join("file"))?, b"captured\n");
    let warmed = fixture.fork("warmed")?;
    assert_eq!(fs::read(warmed.path.join("target/cache"))?, b"built");
    assert_eq!(fs::read(fixture.source.join("file"))?, b"original\n");
    Ok(())
}

#[test]
fn private_checkpoints_survive_collection_with_live_descendants() -> TestResult {
    let Some(fixture) = Fixture::new()? else {
        return Ok(());
    };
    let first = fixture.fork("first")?;
    fs::write(first.path.join("file"), b"unpublished")?;
    fs::write(first.path.join("target/cache"), b"private cache")?;
    let node: Node = fixture.call(&["seal", &first.id.get().to_string()])?;
    let child: Leaf = fixture.call(&[
        "fork",
        text(&fixture.directory.path().join("child"))?,
        "--node",
        node.id.as_str(),
    ])?;
    let _: () = fixture.call(&["release", node.id.as_str()])?;
    fixture.command(&["collect"]).output().map(|output| assert!(output.status.success()))?;
    assert_eq!(fs::read(child.path.join("file"))?, b"unpublished");
    assert_eq!(fs::read(child.path.join("target/cache"))?, b"private cache");
    assert!(fixture.store.join("nodes").join(node.id.as_str()).exists());
    Ok(())
}

#[test]
fn checks_cannot_publish_modified_source_or_an_unchecked_batch() -> TestResult {
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
    assert!(
        !fixture.command(&["commit-batch", "--candidate", text(&path)?]).output()?.status.success()
    );
    assert!(
        !fixture
            .command(&[
                "check-batch",
                "--candidate",
                text(&path)?,
                "--",
                "/bin/sh",
                "-c",
                "printf invalid > file"
            ])
            .output()?
            .status
            .success()
    );
    assert!(
        !fixture.command(&["commit-batch", "--candidate", text(&path)?]).output()?.status.success()
    );
    let _: Validation = fixture.call(&[
        "check-batch",
        "--candidate",
        text(&path)?,
        "--",
        "/bin/sh",
        "-c",
        "test \"$(cat file)\" = first && test \"$(cat other)\" = second",
    ])?;
    let receipt: cowtree_metadata::BatchReceipt =
        fixture.call(&["commit-batch", "--candidate", text(&path)?])?;
    assert_eq!(receipt.receipts.len(), 2);
    assert_eq!(receipt.receipts[0].version, receipt.receipts[1].version);
    let fork = fixture.fork("published")?;
    assert_eq!(fs::read(fork.path.join("file"))?, b"first\n");
    assert_eq!(fs::read(fork.path.join("other"))?, b"second\n");
    Ok(())
}

#[cfg(feature = "fault-injection")]
#[test]
fn lost_commit_acknowledgement_recovers_without_overwriting_later_edits() -> TestResult {
    let Some(fixture) = Fixture::new()? else {
        return Ok(());
    };
    let leaf = fixture.fork("first")?;
    fixture.capture(&leaf, "file", b"captured\n")?;
    let _: cowtree_metadata::Candidate = fixture.call(&["prepare", &leaf.id.get().to_string()])?;
    fixture.check(&leaf, "test \"$(cat file)\" = captured")?;
    fs::write(leaf.path.join("file"), b"later edit\n")?;
    let output = fixture
        .command(&["commit", &leaf.id.get().to_string()])
        .env("COWTREE_WORKSPACE_CRASH_AT", "after-publication-commit")
        .output()?;
    assert_eq!(output.status.code(), Some(73));
    let _: Vec<cowtree_metadata::LeafId> = fixture.call(&["recover"])?;
    assert_eq!(fs::read(leaf.path.join("file"))?, b"later edit\n");
    let ready: Vec<Leaf> = fixture.call(&["list"])?;
    let owner = ready.iter().find(|current| current.id == leaf.id).ok_or("leaf lost")?;
    assert!(owner.pending.is_none());
    assert!(owner.last_receipt.is_some());
    let child = fixture.fork("published")?;
    assert_eq!(fs::read(child.path.join("file"))?, b"captured\n");
    Ok(())
}

#[cfg(feature = "fault-injection")]
#[test]
fn interrupted_fork_releases_only_its_owned_allocation() -> TestResult {
    let Some(fixture) = Fixture::new()? else {
        return Ok(());
    };
    let before: Vec<Leaf> = fixture.call(&["list"])?;
    let target = fixture.directory.path().join("interrupted");
    let output = fixture
        .command(&["fork", text(&target)?])
        .env("COWTREE_WORKSPACE_CRASH_AT", "after-leaf-allocation")
        .output()?;
    assert_eq!(output.status.code(), Some(73));
    let _: Vec<cowtree_metadata::LeafId> = fixture.call(&["recover"])?;
    let after: Vec<Leaf> = fixture.call(&["list"])?;
    assert_eq!(before, after);
    assert!(!target.exists());
    assert_eq!(fs::read(fixture.source.join("file"))?, b"original\n");
    Ok(())
}

#[test]
fn validation_lock_survives_coordinator_death_until_the_native_deadline() -> TestResult {
    use std::{
        thread,
        time::{Duration, Instant},
    };
    let Some(fixture) = Fixture::new()? else {
        return Ok(());
    };
    let leaf = fixture.fork("first")?;
    fixture.capture(&leaf, "file", b"captured\n")?;
    let _: cowtree_metadata::Candidate = fixture.call(&["prepare", &leaf.id.get().to_string()])?;
    let marker = fixture.directory.path().join("started");
    let script = format!("printf started > '{}' ; sleep 20", text(&marker)?);
    let mut coordinator = fixture
        .command(&[
            "check",
            &leaf.id.get().to_string(),
            "--timeout",
            "2",
            "--",
            "/bin/sh",
            "-c",
            &script,
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let started = Instant::now();
    while !marker.exists() {
        if started.elapsed() > Duration::from_secs(10) {
            coordinator.kill()?;
            coordinator.wait()?;
            return Err("validation never started".into());
        }
        thread::sleep(Duration::from_millis(20));
    }
    coordinator.kill()?;
    coordinator.wait()?;
    let active: Vec<Leaf> = fixture.call(&["list"])?;
    let check = active
        .iter()
        .find(|leaf| leaf.check_candidate.is_some())
        .ok_or("missing validation leaf")?;
    let output = fixture.command(&["collect"]).output()?;
    assert!(output.status.success());
    assert!(check.path.exists());
    thread::sleep(Duration::from_secs(3));
    let output = fixture.command(&["collect"]).output()?;
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(!check.path.exists());
    assert!(!fixture.command(&["commit", &leaf.id.get().to_string()]).output()?.status.success());
    assert_eq!(fs::read(leaf.path.join("file"))?, b"captured\n");
    Ok(())
}

#[cfg(feature = "fault-injection")]
#[test]
fn failed_tree_flush_cannot_publish_a_ready_workspace() -> TestResult {
    let Some(fixture) = Fixture::new()? else {
        return Ok(());
    };
    let destination = fixture.directory.path().join("failed-store");
    let output = Command::new(env!("CARGO_BIN_EXE_cowtree"))
        .args(["workspace", "--root"])
        .arg(&destination)
        .args(["init", "--source"])
        .arg(&fixture.source)
        .env("COWTREE_WORKSPACE_IO_ERROR_AT", "tree-sync")
        .output()?;
    assert!(!output.status.success());
    assert!(!destination.exists());
    let output = Command::new(env!("CARGO_BIN_EXE_cowtree"))
        .args(["workspace", "--root"])
        .arg(&destination)
        .arg("recover")
        .output()?;
    let recovered: bool = decode(output)?;
    assert!(!recovered);
    assert!(!fixture.directory.path().join(".failed-store.cowtree-init").exists());
    assert_eq!(fs::read(fixture.source.join("file"))?, b"original\n");
    Ok(())
}

#[test]
fn terminal_interrupt_cannot_remove_the_validation_deadline() -> TestResult {
    use nix::{
        sys::signal::{Signal, killpg},
        unistd::Pid,
    };
    use std::{
        os::unix::process::CommandExt,
        thread,
        time::{Duration, Instant},
    };
    let Some(fixture) = Fixture::new()? else {
        return Ok(());
    };
    let leaf = fixture.fork("first")?;
    fixture.capture(&leaf, "file", b"captured\n")?;
    let _: cowtree_metadata::Candidate = fixture.call(&["prepare", &leaf.id.get().to_string()])?;
    let marker = fixture.directory.path().join("validation-pid");
    let mut coordinator = fixture
        .command(&[
            "check",
            &leaf.id.get().to_string(),
            "--timeout",
            "2",
            "--",
            "/bin/sh",
            "-c",
            "printf '%s' $$ > \"$1\"; sleep 20",
            "check",
            text(&marker)?,
        ])
        .process_group(0)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let started = Instant::now();
    while !marker.exists() {
        if started.elapsed() > Duration::from_secs(10) {
            coordinator.kill()?;
            coordinator.wait()?;
            return Err("validation never started".into());
        }
        thread::sleep(Duration::from_millis(20));
    }
    let checker = Pid::from_raw(fs::read_to_string(&marker)?.parse()?);
    killpg(Pid::from_raw(i32::try_from(coordinator.id())?), Signal::SIGINT)?;
    coordinator.wait()?;
    thread::sleep(Duration::from_secs(3));
    let output = fixture.command(&["collect"]).output()?;
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let active: Vec<Leaf> = fixture.call(&["list"])?;
    let orphaned = active.iter().any(|leaf| leaf.check_candidate.is_some());
    if orphaned {
        match killpg(checker, Signal::SIGKILL) {
            Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
            Err(error) => return Err(error.into()),
        }
    }
    assert!(!orphaned, "terminal interrupt killed the supervisor before its deadline");
    Ok(())
}

#[test]
fn completed_validation_retires_background_members_of_its_group() -> TestResult {
    use nix::{
        sys::signal::{Signal, killpg},
        unistd::Pid,
    };
    let Some(fixture) = Fixture::new()? else {
        return Ok(());
    };
    let leaf = fixture.fork("first")?;
    fixture.capture(&leaf, "file", b"captured\n")?;
    let _: cowtree_metadata::Candidate = fixture.call(&["prepare", &leaf.id.get().to_string()])?;
    let marker = fixture.directory.path().join("group-pid");
    let output = fixture
        .command(&[
            "check",
            &leaf.id.get().to_string(),
            "--timeout",
            "2",
            "--",
            "/bin/sh",
            "-c",
            "printf '%s' $$ > \"$1\"; sleep 20 &",
            "check",
            text(&marker)?,
        ])
        .output()?;
    if !output.status.success() {
        let pid = Pid::from_raw(fs::read_to_string(marker)?.parse()?);
        match killpg(pid, Signal::SIGKILL) {
            Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
            Err(error) => return Err(error.into()),
        }
    }
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let _: Validation = decode(output)?;
    let leaves: Vec<Leaf> = fixture.call(&["list"])?;
    assert!(leaves.iter().all(|leaf| leaf.check_candidate.is_none()));
    Ok(())
}
