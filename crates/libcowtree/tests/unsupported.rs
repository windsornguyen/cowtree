// Copyright (c) 2026 Windsor Nguyen

//! Unsupported filesystems fail before creating a branch or worktree registration.

use cowtree::{AddRequest, Branch, add_worktree, inspect_path};
use std::{error::Error, fs, path::PathBuf, process::Command};

#[test]
fn unsupported_filesystems_leave_no_owned_state() -> Result<(), Box<dyn Error>> {
    let explicit = std::env::var_os("COWTREE_UNSUPPORTED_ROOT").map(PathBuf::from);
    let directory = if let Some(root) = &explicit {
        fs::create_dir_all(root)?;
        tempfile::tempdir_in(root)?
    } else {
        tempfile::tempdir()?
    };
    let supported = inspect_path(directory.path())?.supported();
    if explicit.is_some()
        || std::env::var_os("COWTREE_EXPECT_SUPPORTED").is_some_and(|value| value == "0")
    {
        assert!(!supported);
    }
    if supported {
        eprintln!("skip unsupported filesystem case");
        return Ok(());
    }
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    fs::create_dir(&source)?;
    fs::write(source.join("file"), b"original\n")?;
    for argv in [
        vec!["init", "-q"],
        vec!["config", "user.name", "Test"],
        vec!["config", "user.email", "test@example.invalid"],
        vec!["config", "core.autocrlf", "false"],
        vec!["add", "."],
        vec!["-c", "commit.gpgsign=false", "commit", "-qm", "fixture"],
    ] {
        let output = Command::new("git").arg("-C").arg(&source).args(argv).output()?;
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    }
    let mut request = AddRequest::new(target.clone());
    request.source = Some(source.clone());
    request.branch = Branch::New("owned".into());
    let error = add_worktree(&request).err().ok_or("unsupported filesystem was accepted")?;
    assert_eq!(error.code(), "cow_unavailable");
    assert!(!target.exists());
    let branch = Command::new("git")
        .arg("-C")
        .arg(&source)
        .args(["show-ref", "--verify", "--quiet", "refs/heads/owned"])
        .output()?;
    assert_eq!(branch.status.code(), Some(1));
    assert_eq!(fs::read(source.join("file"))?, b"original\n");
    Ok(())
}
