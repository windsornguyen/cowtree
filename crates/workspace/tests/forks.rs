// Copyright (c) 2026 Windsor Nguyen

//! Managed forks register ordinary Git worktrees with private source and cache writes.

mod common;

use common::{TestResult, fixture, git};
use std::fs;

#[test]
fn forks_preserve_source_and_cache_isolation() -> TestResult {
    let Some(fixture) = fixture()? else {
        return Ok(());
    };
    let first = fixture.workspace.fork(&fixture.directory.path().join("first"), None)?;
    let second = fixture.workspace.fork(&fixture.directory.path().join("second"), None)?;
    assert_ne!(first.id, second.id);
    assert_eq!(fs::read(first.path.join("target/cache"))?, b"warm cache");
    assert!(git(&first.path, &["status", "--porcelain"])?.is_empty());
    fs::write(first.path.join("file"), b"private source")?;
    fs::write(first.path.join("target/cache"), b"private cache")?;
    assert_eq!(fs::read(second.path.join("file"))?, b"original\n");
    assert_eq!(fs::read(second.path.join("target/cache"))?, b"warm cache");
    assert_eq!(fs::read(fixture.source.join("file"))?, b"original\n");
    assert_eq!(fs::read(fixture.source.join("target/cache"))?, b"warm cache");
    let registered = git(&fixture.source, &["worktree", "list", "--porcelain"])?;
    assert!(registered.contains(first.path.to_str().ok_or("non-UTF-8 test path")?));
    assert!(registered.contains(second.path.to_str().ok_or("non-UTF-8 test path")?));
    Ok(())
}

#[test]
fn forks_reject_existing_or_nested_destinations() -> TestResult {
    let Some(fixture) = fixture()? else {
        return Ok(());
    };
    let first = fixture.workspace.fork(&fixture.directory.path().join("first"), None)?;
    assert!(fixture.workspace.fork(&first.path, None).is_err());
    assert!(fixture.workspace.fork(&first.path.join("nested"), None).is_err());
    assert!(fixture.workspace.fork(&fixture.workspace.root.join("nested"), None).is_err());
    assert_eq!(fs::read(first.path.join("file"))?, b"original\n");
    assert!(!first.path.join("nested").exists());
    Ok(())
}
