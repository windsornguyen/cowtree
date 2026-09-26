// Copyright (c) 2026 Windsor Nguyen

//! Preserve admission, private-write, and collection contracts at the native CLI.

#![cfg(unix)]

mod support;

use cowtree_workspace::{Leaf, Pending};
use std::{fs, process::Command};
use support::{Fixture, TestResult, text};

#[test]
fn git_filters_cannot_change_the_recorded_checkpoint_bytes() -> TestResult {
    use std::os::unix::fs::PermissionsExt;
    #[derive(serde::Deserialize)]
    struct Failure {
        code: String,
    }
    let Some(fixture) = Fixture::new()? else {
        return Ok(());
    };
    let leaf = fixture.fork("first")?;
    fs::write(leaf.path.join(".gitattributes"), b"file filter=mutate\n")?;
    let filter = fixture.directory.path().join("filter.sh");
    fs::write(&filter, b"#!/bin/sh\ncat\nprintf 'changed by filter\\n' > file\n")?;
    fs::set_permissions(&filter, fs::Permissions::from_mode(0o755))?;
    let configured = Command::new("git")
        .arg("-C")
        .arg(&fixture.source)
        .args(["config", "filter.mutate.clean"])
        .arg(&filter)
        .status()?;
    assert!(configured.success());
    let result = fixture.command(&["seal", &leaf.id.get().to_string()]).output()?;
    assert!(!result.status.success(), "changed checkpoint was accepted");
    let error: Failure = serde_json::from_slice(&result.stderr)?;
    assert_eq!(error.code, "command_failed");
    let recorded: Leaf = serde_json::from_slice(&fs::read(
        fixture.store.join("leaves").join(format!("{}.json", leaf.id.get())),
    )?)?;
    assert_eq!(recorded.node, leaf.node);
    assert_eq!(fs::read(leaf.path.join("file"))?, b"original\n");
    Ok(())
}

#[test]
fn abort_preserves_private_files_and_discard_uses_the_retained_origin() -> TestResult {
    let Some(fixture) = Fixture::new()? else {
        return Ok(());
    };
    let leaf = fixture.fork("first")?;
    fixture.capture(&leaf, "file", b"captured\n")?;
    fs::write(leaf.path.join("file"), b"later\n")?;
    let _: Leaf = fixture.call(&["discard", &leaf.id.get().to_string(), "file"])?;
    assert_eq!(fs::read(leaf.path.join("file"))?, b"captured\n");
    fs::write(leaf.path.join("file"), b"later\n")?;
    let _: () = fixture.call(&["abort", &leaf.id.get().to_string()])?;
    assert_eq!(fs::read(leaf.path.join("file"))?, b"later\n");
    let _: Leaf = fixture.call(&["discard", &leaf.id.get().to_string(), "file"])?;
    assert_eq!(fs::read(leaf.path.join("file"))?, b"original\n");
    Ok(())
}

#[test]
fn sync_does_not_move_an_attached_branch_or_overwrite_private_source() -> TestResult {
    let Some(fixture) = Fixture::new()? else {
        return Ok(());
    };
    let leaf = fixture.fork("first")?;
    fs::write(leaf.path.join("file"), b"private")?;
    let _: Leaf = fixture.call(&["sync", &leaf.id.get().to_string()])?;
    assert_eq!(fs::read(leaf.path.join("file"))?, b"private");
    let status =
        Command::new("git").arg("-C").arg(&leaf.path).args(["switch", "-c", "caller"]).status()?;
    assert!(status.success());
    assert!(!fixture.command(&["sync", &leaf.id.get().to_string()]).output()?.status.success());
    assert_eq!(fs::read(leaf.path.join("file"))?, b"private");
    Ok(())
}

#[test]
fn replaced_leaf_directories_and_unknown_journals_are_preserved() -> TestResult {
    let Some(fixture) = Fixture::new()? else {
        return Ok(());
    };
    let leaf = fixture.fork("first")?;
    let original = fixture.directory.path().join("original-leaf");
    fs::rename(&leaf.path, &original)?;
    fs::create_dir(&leaf.path)?;
    fs::write(leaf.path.join("sentinel"), b"other owner")?;
    assert!(
        !fixture
            .command(&["drop", &leaf.id.get().to_string(), "--force"])
            .output()?
            .status
            .success()
    );
    assert_eq!(fs::read(leaf.path.join("sentinel"))?, b"other owner");
    fs::create_dir(fixture.store.join("operations/unrecognized"))?;
    assert!(!fixture.command(&["collect"]).output()?.status.success());
    assert!(fixture.store.join("operations/unrecognized").exists());
    Ok(())
}

#[cfg(feature = "fault-injection")]
#[test]
fn lost_capture_acknowledgement_reuses_the_frozen_bytes() -> TestResult {
    let Some(fixture) = Fixture::new()? else {
        return Ok(());
    };
    let leaf = fixture.fork("first")?;
    fs::write(leaf.path.join("file"), b"frozen\n")?;
    let output = fixture
        .command(&["capture", &leaf.id.get().to_string()])
        .env("COWTREE_WORKSPACE_CRASH_AT", "after-capture-intent")
        .output()?;
    assert_eq!(output.status.code(), Some(73));
    fs::write(leaf.path.join("file"), b"later\n")?;
    let _: Vec<cowtree_metadata::LeafId> = fixture.call(&["recover"])?;
    let leaves: Vec<Leaf> = fixture.call(&["list"])?;
    let pending: &Pending = leaves
        .iter()
        .find(|current| current.id == leaf.id)
        .and_then(|current| current.pending.as_ref())
        .ok_or("capture lost")?;
    assert!(pending.submitted);
    let _: cowtree_metadata::Candidate = fixture.call(&["prepare", &leaf.id.get().to_string()])?;
    fixture.check(&leaf, "test \"$(cat file)\" = frozen")?;
    let _: cowtree_metadata::Receipt = fixture.call(&["commit", &leaf.id.get().to_string()])?;
    assert_eq!(fs::read(leaf.path.join("file"))?, b"later\n");
    Ok(())
}

#[test]
fn pinned_dependencies_are_materialized_read_only() -> TestResult {
    let Some(fixture) = Fixture::new()? else {
        return Ok(());
    };
    add_dependency(&fixture)?;
    let store = fixture.directory.path().join("pinned-store");
    let binary = env!("CARGO_BIN_EXE_cowtree");
    let init = Command::new(binary)
        .args(["workspace", "--root"])
        .arg(&store)
        .args(["init", "--source"])
        .arg(&fixture.source)
        .args(["--submodules", "materialize-pinned", "--derived", "target"])
        .output()?;
    assert!(init.status.success(), "{}", String::from_utf8_lossy(&init.stderr));
    let target = fixture.directory.path().join("pinned-leaf");
    let output = Command::new(binary)
        .args(["workspace", "--root"])
        .arg(&store)
        .arg("fork")
        .arg(&target)
        .output()?;
    let leaf: Leaf = support::decode(output)?;
    assert_eq!(fs::read(target.join("vendor/dependency/dependency.txt"))?, b"pinned");
    assert!(!target.join("vendor/dependency/.git").exists());
    assert_eq!(
        fs::read_link(target.join("vendor/dependency/relative-link"))?,
        std::path::Path::new("future-file")
    );
    assert!(target.join("vendor/dependency/.cowtree-pin").exists());
    let denied = Command::new(binary)
        .args([
            "workspace",
            "--root",
            text(&store)?,
            "acquire",
            &leaf.id.get().to_string(),
            "vendor",
        ])
        .output()?;
    assert!(!denied.status.success());
    fs::write(target.join("vendor/dependency/dependency.txt"), b"changed")?;
    let denied = Command::new(binary)
        .args(["workspace", "--root", text(&store)?, "capture", &leaf.id.get().to_string()])
        .output()?;
    assert!(!denied.status.success());
    Ok(())
}

fn add_dependency(fixture: &Fixture) -> TestResult {
    let child = fixture.directory.path().join("dependency");
    fs::create_dir(&child)?;
    fs::write(child.join("dependency.txt"), b"pinned")?;
    std::os::unix::fs::symlink("future-file", child.join("relative-link"))?;
    for argv in [
        vec!["init", "-q"],
        vec!["config", "user.name", "Test"],
        vec!["config", "user.email", "test@example.invalid"],
        vec!["add", "."],
        vec!["-c", "commit.gpgsign=false", "commit", "-qm", "dependency"],
    ] {
        assert!(Command::new("git").arg("-C").arg(&child).args(argv).output()?.status.success());
    }
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&fixture.source)
            .args(["-c", "protocol.file.allow=always", "submodule", "add", "-q"])
            .arg(&child)
            .arg("vendor/dependency")
            .output()?
            .status
            .success()
    );
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&fixture.source)
            .args(["commit", "-qam", "pin dependency"])
            .output()?
            .status
            .success()
    );
    Ok(())
}
