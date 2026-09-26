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
