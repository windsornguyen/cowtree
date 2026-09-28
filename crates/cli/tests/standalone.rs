// Copyright (c) 2026 Windsor Nguyen

//! The shipped executables preserve standalone argv, JSON, and isolation contracts.

use serde::Deserialize;
use std::{fs, path::PathBuf, process::Command};

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Deserialize)]
struct Worktree {
    /// Registered filesystem location.
    path: PathBuf,
    /// Attached branch, when requested.
    branch: Option<String>,
}

#[derive(Deserialize)]
struct Reply<T> {
    /// Explicit success status.
    status: String,
    /// Typed operation value.
    value: T,
}

#[test]
fn both_executables_report_native_build_identity() -> Result {
    for binary in [env!("CARGO_BIN_EXE_cowtree"), env!("CARGO_BIN_EXE_git-cowtree")] {
        let output = Command::new(binary).args(["--version", "--json"]).output()?;
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout)?;
        assert!(text.contains("\"version\""));
        assert!(text.contains("\"revision\""));
    }
    Ok(())
}

#[test]
fn invalid_options_fail_before_mutating_a_repository() -> Result {
    let directory = tempfile::tempdir()?;
    for args in [
        vec!["--json", "add", "--force", "new"],
        vec!["--json", "add", "-b", "new", "--detach", "new"],
        vec!["--json", "add", "--reason", "reason", "new"],
        vec!["--json", "--version", "list"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_cowtree"))
            .args(args)
            .current_dir(directory.path())
            .output()?;
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8(output.stderr)?.contains("invalid_arguments"));
        assert!(!directory.path().join("new").exists());
    }
    Ok(())
}

#[test]
fn standalone_clones_are_registered_private_and_removable() -> Result {
    let directory = tempfile::tempdir()?;
    let supported = cowtree::inspect_path(directory.path())?.supported();
    if std::env::var_os("COWTREE_EXPECT_SUPPORTED").is_some_and(|value| value == "1") {
        assert!(supported);
    }
    if !supported {
        eprintln!("skip native filesystem");
        return Ok(());
    }
    let source = directory.path().join("source");
    fs::create_dir(&source)?;
    fs::write(source.join("file"), b"original\n")?;
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.name", "Test"],
        vec!["config", "user.email", "test@example.invalid"],
        vec!["config", "core.autocrlf", "false"],
        vec!["add", "."],
        vec!["-c", "commit.gpgsign=false", "commit", "-qm", "fixture"],
    ] {
        assert!(Command::new("git").arg("-C").arg(&source).args(args).output()?.status.success());
    }
    let target = directory.path().join("target");
    let binary = env!("CARGO_BIN_EXE_cowtree");
    let output = Command::new(binary)
        .args(["--json", "add", "-b", "private"])
        .arg(&target)
        .current_dir(&source)
        .output()?;
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let reply: Reply<Worktree> = serde_json::from_slice(&output.stdout)?;
    assert_eq!(reply.status, "ok");
    assert_eq!(reply.value.path.canonicalize()?, target.canonicalize()?);
    assert_eq!(reply.value.branch.as_deref(), Some("refs/heads/private"));
    fs::write(target.join("file"), b"private edit")?;
    assert_eq!(fs::read(source.join("file"))?, b"original\n");
    assert!(
        !Command::new(binary)
            .arg("remove")
            .arg(&target)
            .current_dir(&source)
            .output()?
            .status
            .success()
    );
    assert!(
        Command::new(binary)
            .args(["remove", "--force"])
            .arg(&target)
            .current_dir(&source)
            .output()?
            .status
            .success()
    );
    assert!(!target.exists());
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&source)
            .args(["show-ref", "--verify", "refs/heads/private"])
            .output()?
            .status
            .success()
    );
    Ok(())
}

/// Run Git in `root` and return its trimmed standard output.
fn git(root: &std::path::Path, arguments: &[&str]) -> Result<String> {
    let output = Command::new("git").arg("-C").arg(root).args(arguments).output()?;
    assert!(output.status.success(), "{arguments:?}: {}", String::from_utf8_lossy(&output.stderr));
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

/// Invariant: a committed add in a blobless clone fetches the commit's missing blobs in one
/// explicit request and never depends on Git's lazy one-blob-per-request fetch, which takes
/// minutes on a large repository.
/// Witness: with `GIT_NO_LAZY_FETCH=1`, the add of a commit whose new blob is absent locally
/// succeeds and writes that blob's bytes.
#[test]
fn committed_add_in_a_blobless_clone_fetches_missing_blobs_at_once() -> Result {
    let directory = tempfile::tempdir()?;
    if !cowtree::inspect_path(directory.path())?.supported() {
        eprintln!("skip native filesystem");
        return Ok(());
    }
    let origin = directory.path().join("origin");
    fs::create_dir(&origin)?;
    git(&origin, &["init", "-q", "-b", "main"])?;
    for (key, value) in [
        ("user.name", "Test"),
        ("user.email", "test@example.invalid"),
        ("commit.gpgsign", "false"),
        ("uploadpack.allowFilter", "true"),
        ("uploadpack.allowAnySHA1InWant", "true"),
    ] {
        git(&origin, &["config", key, value])?;
    }
    fs::write(origin.join("file"), b"base\n")?;
    git(&origin, &["add", "."])?;
    git(&origin, &["commit", "-qm", "base"])?;

    let url = format!("file://{}", origin.display());
    let clone = directory.path().join("clone");
    let output = Command::new("git")
        .args(["clone", "-q", "--filter=blob:none", "-c", "core.autocrlf=false", &url])
        .arg(&clone)
        .output()?;
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));

    fs::write(origin.join("added"), b"only in the next commit\n")?;
    git(&origin, &["add", "."])?;
    git(&origin, &["commit", "-qm", "next"])?;
    git(&clone, &["fetch", "-q", "origin"])?;
    let next = git(&clone, &["rev-parse", "origin/main"])?;
    let missing = git(&clone, &["rev-list", "--objects", "--no-walk", "--missing=print", &next])?;
    assert!(missing.lines().any(|line| line.starts_with('?')), "fixture must lack a blob");

    let target = directory.path().join("tree");
    let output = Command::new(env!("CARGO_BIN_EXE_cowtree"))
        .current_dir(&clone)
        .env("GIT_NO_LAZY_FETCH", "1")
        .args(["add", "--committed", "-d"])
        .arg(&target)
        .arg(&next)
        .output()?;
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(fs::read(target.join("added"))?, b"only in the next commit\n");
    Ok(())
}
