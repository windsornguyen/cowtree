// Copyright (c) 2026 Windsor Nguyen

//! Real Git transactions preserve source edits, private worktrees, and branch ownership.

use cowtree::{
    AddRequest, Branch, Lock, SourceMode, add_worktree, inspect_path, list_worktrees,
    remove_worktree,
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
    git(&source, &["config", "core.autocrlf", "false"])?;
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
fn creation_reports_the_published_git_lock_state() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    if !native(directory.path())? {
        return Ok(());
    }
    let source = repository(directory.path())?;
    for (index, lock) in [
        Lock::Release,
        Lock::Retain { reason: None },
        Lock::Retain { reason: Some("owned checkpoint".into()) },
    ]
    .into_iter()
    .enumerate()
    {
        let mut request = AddRequest::new(directory.path().join(format!("target-{index}")));
        request.source = Some(source.clone());
        request.lock = lock;
        let created = add_worktree(&request)?;
        let listed = list_worktrees(Some(&source))?
            .into_iter()
            .find(|tree| tree.path == created.path)
            .ok_or("created worktree is absent")?;
        assert_eq!(created, listed);
    }
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
fn committed_fork_applies_checkout_conversions_without_changing_source()
-> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    if !native(directory.path())? {
        return Ok(());
    }
    let source = repository(directory.path())?;
    fs::write(source.join(".gitattributes"), b"file text eol=crlf\n")?;
    git(&source, &["add", ".gitattributes"])?;
    git(&source, &["commit", "-qm", "checkout conversion"])?;
    fs::write(source.join("file"), b"private edit\n")?;
    let mut request = AddRequest::new(directory.path().join("converted"));
    request.source = Some(source.clone());
    request.source_mode = SourceMode::Committed;
    let tree = add_worktree(&request)?;
    assert_eq!(fs::read(tree.path.join("file"))?, b"original\r\n");
    assert_eq!(fs::read(source.join("file"))?, b"private edit\n");
    assert_eq!(git(&tree.path, &["status", "--porcelain"])?, "");
    remove_worktree(&tree.path, Some(&source), false)?;
    Ok(())
}

#[cfg(unix)]
#[test]
fn committed_checkout_runs_hooks_in_the_final_worktree() -> Result<(), Box<dyn Error>> {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir()?;
    if !native(directory.path())? {
        return Ok(());
    }
    let source = repository(directory.path())?;
    let hook = source.join(".git/hooks/post-checkout");
    fs::write(&hook, b"#!/bin/sh\nprintf '%s\\n' \"$PWD\" > \"$(git rev-parse --path-format=absolute --git-common-dir)/hook-target\"\n")?;
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755))?;
    let mut request = AddRequest::new(directory.path().join("final tree"));
    request.source = Some(source.clone());
    request.source_mode = SourceMode::Committed;
    let tree = add_worktree(&request)?;
    let observed = fs::read_to_string(source.join(".git/hook-target"))?;
    assert_eq!(observed.trim_end(), tree.path.to_str().ok_or("non-UTF-8 test path")?);
    assert!(!git(&source, &["worktree", "list", "--porcelain"])?.contains(".cowtree-seed-"));
    Ok(())
}

#[cfg(unix)]
#[test]
fn committed_checkout_rejects_hook_edits_hidden_by_stat_settings() -> Result<(), Box<dyn Error>> {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir()?;
    if !native(directory.path())? {
        return Ok(());
    }
    let source = repository(directory.path())?;
    git(&source, &["config", "core.trustctime", "false"])?;
    git(&source, &["config", "core.checkstat", "minimal"])?;
    let hook = source.join(".git/hooks/post-checkout");
    fs::write(&hook, b"#!/bin/sh\nset -e\ntouch -t 200001010000 file\ngit update-index --refresh\nprintf 'modified\\n' > file\ntouch -t 200001010000 file\ntest -z \"$(git status --porcelain)\"\n")?;
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755))?;
    let mut request = AddRequest::new(directory.path().join("rejected"));
    request.source = Some(source.clone());
    request.source_mode = SourceMode::Committed;
    request.branch = Branch::New("owned".into());
    let error = add_worktree(&request).err().ok_or("hook edits were accepted")?;
    assert_eq!(error.code(), "dirty_source");
    assert!(!request.path.exists());
    assert!(git(&source, &["branch", "--list", "owned"])?.is_empty());
    assert_eq!(fs::read(source.join("file"))?, b"original\n");
    Ok(())
}

#[test]
fn committed_paths_do_not_alias_on_the_destination_filesystem() -> Result<(), Box<dyn Error>> {
    for names in [["Alias", "alias"], ["Dir/one", "dir/two"]] {
        let directory = tempfile::tempdir()?;
        if !native(directory.path())? {
            return Ok(());
        }
        fs::write(directory.path().join("CaseProbe"), b"probe")?;
        let aliases = directory.path().join("caseprobe").exists();
        let source = repository(directory.path())?;
        let blob = git(&source, &["rev-parse", "HEAD:file"])?;
        git(&source, &["read-tree", "--empty"])?;
        for name in names {
            git(
                &source,
                &[
                    "-c",
                    "core.ignorecase=false",
                    "update-index",
                    "--add",
                    "--cacheinfo",
                    "100644",
                    blob.trim(),
                    name,
                ],
            )?;
        }
        let tree = git(&source, &["write-tree"])?;
        let commit = git(&source, &["commit-tree", tree.trim(), "-p", "HEAD", "-m", "names"])?;
        let mut request = AddRequest::new(directory.path().join("target"));
        request.source = Some(source.clone());
        request.source_mode = SourceMode::Committed;
        request.revision = commit.trim().into();
        let created = add_worktree(&request);
        if aliases {
            assert!(created.is_err(), "aliased paths accepted");
            assert!(!request.path.exists());
        } else {
            let created = created?;
            for name in names {
                assert_eq!(fs::read(created.path.join(name))?, b"original\n");
            }
            remove_worktree(&created.path, Some(&source), false)?;
        }
        assert_eq!(fs::read(source.join("file"))?, b"original\n");
    }
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

#[cfg(unix)]
#[test]
fn creation_rejects_source_bytes_hidden_by_stat_cache() -> Result<(), Box<dyn Error>> {
    use std::{
        fs::FileTimes,
        time::{Duration, UNIX_EPOCH},
    };
    let root = tempfile::tempdir()?;
    if !native(root.path())? {
        return Ok(());
    }
    let source = repository(root.path())?;
    let file = source.join("file");
    let original = fs::read(&file)?;
    let timestamp = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    git(&source, &["config", "core.trustctime", "false"])?;
    git(&source, &["config", "core.checkstat", "minimal"])?;
    fs::File::open(&file)?.set_times(FileTimes::new().set_modified(timestamp))?;
    git(&source, &["update-index", "--refresh"])?;
    let changed: Vec<_> = original.iter().map(|byte| byte.wrapping_add(1)).collect();
    fs::write(&file, &changed)?;
    fs::File::open(&file)?.set_times(FileTimes::new().set_modified(timestamp))?;
    assert!(git(&source, &["status", "--porcelain"])?.is_empty());
    let target = root.path().join("target");
    let mut request = AddRequest::new(target.clone());
    request.source = Some(source.clone());
    request.branch = Branch::New("owned".into());
    let error = add_worktree(&request).err().ok_or("changed source accepted")?;
    assert_eq!(error.code(), "dirty_source");
    assert!(!target.exists());
    assert_eq!(fs::read(&file)?, changed);
    Ok(())
}
