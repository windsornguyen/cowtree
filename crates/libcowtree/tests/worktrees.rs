// Copyright (c) 2026 Windsor Nguyen

//! Real Git transactions preserve source edits, private worktrees, and branch ownership.

use cowtree::{
    AddRequest, Branch, SourceMode, add_worktree, inspect_path, list_worktrees, remove_worktree,
};
use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn git(root: &Path, arguments: &[&str]) -> Result<String, Box<dyn Error>> {
    let result = Command::new("git").arg("-C").arg(root).args(arguments).output()?;
    assert!(
        result.status.success(),
        "{:?}: {}",
        arguments,
        String::from_utf8_lossy(&result.stderr)
    );
    Ok(String::from_utf8(result.stdout)?)
}

fn repository(root: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let source = root.join("repository");
    fs::create_dir(&source)?;
    git(&source, &["init", "-q", "-b", "source"])?;
    git(&source, &["config", "user.name", "Cowtree test"])?;
    git(&source, &["config", "user.email", "test@example.invalid"])?;
    git(&source, &["config", "commit.gpgsign", "false"])?;
    fs::write(source.join("file"), b"original\n")?;
    git(&source, &["add", "file"])?;
    git(&source, &["commit", "-qm", "fixture"])?;
    Ok(source)
}

fn native(root: &Path) -> Result<bool, Box<dyn Error>> {
    let report = inspect_path(root)?;
    if std::env::var_os("COWTREE_EXPECT_SUPPORTED").is_some_and(|value| value == "1") {
        assert!(report.supported(), "{:?}", report.reason);
    }
    if !report.supported() {
        eprintln!("skip native filesystem: {:?}", report.reason);
    }
    Ok(report.supported())
}

#[test]
fn worktree_lifecycle_preserves_source_and_branch() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    if !native(directory.path())? {
        return Ok(());
    }
    let source = repository(directory.path())?;
    let mut request = AddRequest::new(directory.path().join("agent tree"));
    request.source = Some(source.clone());
    request.branch = Branch::New("agent/task".into());
    let tree = add_worktree(&request)?;
    assert_eq!(tree.branch.as_deref(), Some("refs/heads/agent/task"));
    assert!(list_worktrees(Some(&source))?.iter().any(|entry| entry.path == tree.path));
    assert_eq!(git(&tree.path, &["status", "--porcelain"])?, "");
    fs::write(tree.path.join("file"), b"agent edit\n")?;
    assert_eq!(fs::read(source.join("file"))?, b"original\n");
    assert!(remove_worktree(&tree.path, Some(&source), false).is_err());
    assert_eq!(fs::read(tree.path.join("file"))?, b"agent edit\n");
    remove_worktree(&tree.path, Some(&source), true)?;
    assert!(!tree.path.exists());
    assert_eq!(git(&source, &["rev-parse", "agent/task"])?, git(&source, &["rev-parse", "HEAD"])?);
    Ok(())
}

#[test]
fn committed_fork_preserves_staged_and_unstaged_edits() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    if !native(directory.path())? {
        return Ok(());
    }
    let source = repository(directory.path())?;
    fs::write(source.join("file"), b"staged edit\n")?;
    git(&source, &["add", "file"])?;
    fs::write(source.join("file"), b"unstaged edit\n")?;
    let index = git(&source, &["diff", "--cached"])?;
    let mut request = AddRequest::new(directory.path().join("committed"));
    request.source = Some(source.clone());
    assert!(add_worktree(&request).is_err());
    assert!(!request.path.exists());
    request.source_mode = SourceMode::Committed;
    let tree = add_worktree(&request)?;
    assert!(tree.detached);
    assert_eq!(fs::read(tree.path.join("file"))?, b"original\n");
    assert_eq!(fs::read(source.join("file"))?, b"unstaged edit\n");
    assert_eq!(git(&source, &["diff", "--cached"])?, index);
    assert_eq!(list_worktrees(Some(&source))?.len(), 2);
    remove_worktree(&tree.path, Some(&source), false)?;
    Ok(())
}

#[test]
fn existing_branch_is_not_owned_by_failed_creation() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    if !native(directory.path())? {
        return Ok(());
    }
    let source = repository(directory.path())?;
    let before = git(&source, &["rev-parse", "HEAD"])?;
    let mut request = AddRequest::new(directory.path().join("rejected"));
    request.source = Some(source.clone());
    request.branch = Branch::New("source".into());
    assert!(add_worktree(&request).is_err());
    assert!(!request.path.exists());
    assert_eq!(git(&source, &["rev-parse", "source"])?, before);
    assert_eq!(list_worktrees(Some(&source))?.len(), 1);
    Ok(())
}

#[test]
fn occupied_branch_failure_does_not_fabricate_a_cleanup_failure() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    if !native(directory.path())? {
        return Ok(());
    }
    let source = repository(directory.path())?;
    let mut request = AddRequest::new(directory.path().join("occupied"));
    request.source = Some(source.clone());
    request.branch = Branch::Existing("source".into());
    let error = add_worktree(&request).err().ok_or("occupied branch unexpectedly attached")?;
    assert_eq!(error.code(), "command_failed");
    assert!(!request.path.exists());
    assert_eq!(list_worktrees(Some(&source))?.len(), 1);
    Ok(())
}

#[test]
fn nested_worktrees_copy_only_pinned_files() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    if !native(directory.path())? {
        return Ok(());
    }
    let source = repository(directory.path())?;
    let mut request = AddRequest::new(source.join(".agents/task"));
    request.source = Some(source.clone());
    let tree = add_worktree(&request)?;
    assert_eq!(fs::read(tree.path.join("file"))?, b"original\n");
    assert!(!tree.path.join(".agents").exists());
    assert_eq!(git(&tree.path, &["status", "--porcelain"])?, "");
    remove_worktree(&tree.path, Some(&source), false)?;
    Ok(())
}
