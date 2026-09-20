// Copyright (c) 2026 Windsor Nguyen

//! Interrupt real transactions at ownership transitions, then inspect Git and disk.

use super::*;
use std::{error::Error as StdError, io};

type TestResult = std::result::Result<(), Box<dyn StdError>>;

fn repository(parent: &Path) -> Result<Git> {
    let root = parent.join("repository");
    fs::create_dir(&root).map_err(|error| Error::io(&root, error))?;
    let git = Git { root };
    for arguments in [
        vec!["init", "-q", "-b", "source"],
        vec!["config", "user.name", "Cowtree test"],
        vec!["config", "user.email", "test@example.invalid"],
        vec!["config", "commit.gpgsign", "false"],
    ] {
        git.capture(git.command().args(arguments))?;
    }
    fs::write(git.root.join("file"), b"original\n").map_err(|error| Error::io(&git.root, error))?;
    git.capture(git.command().args(["add", "file"]))?;
    git.capture(git.command().args(["commit", "-qm", "fixture"]))?;
    Ok(git)
}

fn supported(path: &Path) -> Result<bool> {
    let report = inspect_path(path)?;
    if std::env::var_os("COWTREE_EXPECT_SUPPORTED").is_some_and(|value| value == "1") {
        assert!(report.supported(), "{:?}", report.reason);
    }
    if !report.supported() {
        eprintln!("skip native filesystem: {:?}", report.reason);
    }
    Ok(report.supported())
}

fn injected(message: &'static str) -> Error {
    Error::io(Path::new("injected"), io::Error::other(message))
}

fn has_branch(git: &Git, name: &str) -> Result<bool> {
    let references = git.text(git.command().args(["for-each-ref", "--format=%(refname)"]))?;
    Ok(references.lines().any(|reference| reference == format!("refs/heads/{name}")))
}

#[test]
fn failed_creation_removes_only_owned_state() -> TestResult {
    let root = tempfile::tempdir()?;
    if !supported(root.path())? {
        return Ok(());
    }
    let git = repository(root.path())?;
    fs::write(root.path().join("sentinel"), b"caller data")?;
    for phase in [Phase::BeforeRegister, Phase::BranchCreated, Phase::BeforeCopy] {
        for locked in [false, true] {
            for name in ["target", "cafe\u{301}"] {
                let mut request = AddRequest::new(root.path().join("owned/nested").join(name));
                request.branch = Branch::New("owned".into());
                if locked {
                    request.lock = Lock::Retain { reason: None };
                }
                let hook = |current| {
                    if phase == current {
                        return Err(injected("creation failure"));
                    }
                    Ok(())
                };
                let mut creation = Creation::new(&git, &request)?;
                creation.hook = Some(&hook);
                let error = creation.execute().err().ok_or("creation unexpectedly succeeded")?;
                assert_eq!(error.code(), "command_failed");
                assert!(!root.path().join("owned").exists());
                assert!(!has_branch(&git, "owned")?);
                assert_eq!(git.worktrees()?.len(), 1);
                assert_eq!(fs::read(root.path().join("sentinel"))?, b"caller data");
            }
        }
    }
    Ok(())
}

#[test]
fn cleanup_failures_identify_retained_resources_and_both_causes() -> TestResult {
    for failure in [Phase::BeforeRemove, Phase::BeforeDeleteRef] {
        let root = tempfile::tempdir()?;
        if !supported(root.path())? {
            return Ok(());
        }
        let git = repository(root.path())?;
        let mut request = AddRequest::new(root.path().join("target"));
        request.branch = Branch::New("owned".into());
        let hook = |phase| {
            if phase == Phase::BeforeCopy {
                return Err(injected("clone failure"));
            }
            if phase == failure {
                return Err(injected("cleanup failure"));
            }
            Ok(())
        };
        let mut creation = Creation::new(&git, &request)?;
        creation.hook = Some(&hook);
        let error = creation.execute().err().ok_or("creation unexpectedly succeeded")?;
        assert_eq!(error.code(), "cleanup_failed");
        for text in ["clone failure", "cleanup failure", "owned", "target"] {
            assert!(error.to_string().contains(text), "{error}");
        }
        assert_eq!(request.path.exists(), failure == Phase::BeforeRemove);
        assert!(has_branch(&git, "owned")?);
    }
    Ok(())
}

#[test]
fn source_changes_cannot_publish_different_contents_or_head() -> TestResult {
    for advance_head in [false, true] {
        let root = tempfile::tempdir()?;
        if !supported(root.path())? {
            return Ok(());
        }
        let git = repository(root.path())?;
        let mut request = AddRequest::new(root.path().join("target"));
        request.branch = Branch::New("owned".into());
        let hook = |phase| {
            if phase == Phase::BeforeCopy {
                if advance_head {
                    git.capture(git.command().args(["commit", "--allow-empty", "-qm", "advance"]))?;
                } else {
                    fs::write(git.root.join("file"), b"changed during clone\n")
                        .map_err(|error| Error::io(&git.root, error))?;
                }
            }
            Ok(())
        };
        let mut creation = Creation::new(&git, &request)?;
        creation.hook = Some(&hook);
        let error = creation.execute().err().ok_or("creation unexpectedly succeeded")?;
        assert_eq!(error.code(), if advance_head { "head_mismatch" } else { "dirty_source" });
        assert!(!request.path.exists());
        assert!(!has_branch(&git, "owned")?);
        assert_eq!(git.worktrees()?.len(), 1);
    }
    Ok(())
}

#[test]
fn pinned_tree_ignores_new_index_entries_without_losing_them() -> TestResult {
    let root = tempfile::tempdir()?;
    if !supported(root.path())? {
        return Ok(());
    }
    let git = repository(root.path())?;
    let request = AddRequest::new(root.path().join("target"));
    let hook = |phase| {
        if phase == Phase::BeforeCopy {
            fs::write(git.root.join("new"), b"staged during clone")
                .map_err(|error| Error::io(&git.root, error))?;
            git.capture(git.command().args(["add", "new"]))?;
        }
        Ok(())
    };
    let mut creation = Creation::new(&git, &request)?;
    creation.hook = Some(&hook);
    let tree = creation.execute()?;
    assert!(!tree.path.join("new").exists());
    assert_eq!(git.text(git.command().args(["diff", "--cached", "--name-only"]))?, "new");
    assert!(Git { root: tree.path }.status()?.is_empty());
    Ok(())
}

#[test]
fn rollback_preserves_refs_moved_by_other_writers() -> TestResult {
    for existing in [false, true] {
        let root = tempfile::tempdir()?;
        if !supported(root.path())? {
            return Ok(());
        }
        let git = repository(root.path())?;
        let earlier = git.head()?;
        git.capture(git.command().args(["commit", "--allow-empty", "-qm", "advance"]))?;
        let current = git.head()?;
        let mut request = AddRequest::new(root.path().join("target"));
        request.branch = if existing {
            git.capture(git.command().args(["branch", "agent"]))?;
            Branch::Existing("agent".into())
        } else {
            Branch::New("agent".into())
        };
        let hook = |phase| {
            if phase == Phase::BeforeCopy {
                git.capture(git.command().args([
                    "update-ref",
                    "refs/heads/agent",
                    &earlier,
                    &current,
                ]))?;
            }
            Ok(())
        };
        let mut creation = Creation::new(&git, &request)?;
        creation.hook = Some(&hook);
        let error = creation.execute().err().ok_or("creation unexpectedly succeeded")?;
        assert_eq!(error.code(), if existing { "head_mismatch" } else { "cleanup_failed" });
        assert_eq!(git.resolve(OsStr::new("agent"))?, earlier);
        assert_eq!(git.head()?, current);
        assert!(!request.path.exists());
        assert_eq!(git.worktrees()?.len(), 1);
    }
    Ok(())
}

#[test]
fn cancellation_retires_owned_state_before_returning() -> TestResult {
    let root = tempfile::tempdir()?;
    if !supported(root.path())? {
        return Ok(());
    }
    let git = repository(root.path())?;
    let mut request = AddRequest::new(root.path().join("target"));
    request.branch = Branch::New("cancelled".into());
    let hook = |phase| {
        if phase == Phase::BeforeCopy {
            request.cancellation.cancel();
        }
        Ok(())
    };
    let mut creation = Creation::new(&git, &request)?;
    creation.hook = Some(&hook);
    let error = creation.execute().err().ok_or("creation unexpectedly succeeded")?;
    assert_eq!(error.code(), "cancelled");
    assert!(!request.path.exists());
    assert!(!has_branch(&git, "cancelled")?);
    assert_eq!(git.worktrees()?.len(), 1);
    Ok(())
}
