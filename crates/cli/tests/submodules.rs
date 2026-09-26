// Copyright (c) 2026 Windsor Nguyen

//! Standalone submodule policies preserve pins and source isolation.

#[path = "submodule_support/fixture.rs"]
mod fixture;
use fixture::{Failure, Fixture, Result, git, init};
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[test]
fn invariant_uninitialized_submodules_remain_empty_and_pinned() -> Result {
    let Some(fixture) = Fixture::new(false)? else {
        return Ok(());
    };
    for args in [vec![], vec!["--committed"]] {
        let output = fixture.add(&args)?;
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(fs::read_dir(fixture.target().join("vendor/child"))?.next().is_none());
        assert!(
            git(&fixture.target(), &["submodule", "status"])?
                .starts_with(&format!("-{}", fixture.pin))
        );
        let removed = fixture.cowtree(&["remove", fixture.target().to_str().ok_or("path")?])?;
        assert!(removed.status.success(), "{}", String::from_utf8_lossy(&removed.stderr));
    }
    Ok(())
}

#[test]
fn invariant_policy_refusals_leave_no_destination() -> Result {
    let Some(fixture) = Fixture::new(true)? else {
        return Ok(());
    };
    for (args, expected) in [
        (vec![], "submodule_initialized"),
        (vec!["--submodules", "reject"], "submodule_unsupported"),
    ] {
        let output = fixture.add(&args)?;
        assert!(!output.status.success());
        let failure: Failure = serde_json::from_slice(&output.stderr)?;
        assert_eq!(failure.code, expected);
        assert!(failure.message.contains("vendor/child"));
        assert!(!fixture.target().exists());
    }
    Ok(())
}

#[test]
fn invariant_materialized_children_own_their_git_state() -> Result {
    let Some(fixture) = Fixture::new(true)? else {
        return Ok(());
    };
    let output = fixture.add(&["--committed", "--submodules", "materialize-pinned"])?;
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let child = fixture.target().join("vendor/child");
    assert!(
        git(&fixture.target(), &["submodule", "status"])?.starts_with(&format!(" {}", fixture.pin))
    );
    assert!(git(&child, &["status", "--porcelain"])?.is_empty());
    assert!(child.join(".git").is_dir());
    git(&child, &["config", "user.name", "Test"])?;
    git(&child, &["config", "user.email", "test@example.invalid"])?;
    fs::write(child.join("file"), b"private\n")?;
    git(&child, &["-c", "commit.gpgsign=false", "commit", "-qam", "private"])?;
    assert_eq!(git(&fixture.source.join("vendor/child"), &["rev-parse", "HEAD"])?, fixture.pin);
    assert_eq!(fs::read(fixture.source.join("vendor/child/file"))?, b"original\n");
    assert!(
        !fixture.cowtree(&["remove", fixture.target().to_str().ok_or("path")?])?.status.success()
    );
    assert!(
        fixture
            .cowtree(&["remove", "--force", fixture.target().to_str().ok_or("path")?])?
            .status
            .success()
    );
    assert!(!fixture.target().exists());
    Ok(())
}

#[test]
fn invariant_pinned_materialization_uses_committed_bytes_and_local_objects() -> Result {
    let Some(fixture) = Fixture::new(true)? else {
        return Ok(());
    };
    let selected = git(&fixture.source, &["rev-parse", "HEAD"])?;
    let source_child = fixture.source.join("vendor/child");
    git(&source_child, &["config", "user.name", "Test"])?;
    git(&source_child, &["config", "user.email", "test@example.invalid"])?;
    fs::write(source_child.join("file"), b"new commit\n")?;
    git(&source_child, &["-c", "commit.gpgsign=false", "commit", "-qam", "advance"])?;
    git(&fixture.source, &["commit", "-qam", "advance pin"])?;
    let current = git(&source_child, &["rev-parse", "HEAD"])?;
    fs::write(source_child.join("file"), b"private source edits\n")?;
    let target = fixture.target();
    let output = fixture.cowtree(&[
        "add",
        "--json",
        "--committed",
        "--submodules",
        "materialize-pinned",
        target.to_str().ok_or("path")?,
        &selected,
    ])?;
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(git(&target.join("vendor/child"), &["rev-parse", "HEAD"])?, fixture.pin);
    assert_eq!(fs::read(target.join("vendor/child/file"))?, b"original\n");
    assert_eq!(git(&source_child, &["rev-parse", "HEAD"])?, current);
    assert_eq!(fs::read(source_child.join("file"))?, b"private source edits\n");
    let removed = fixture.cowtree(&["remove", target.to_str().ok_or("path")?])?;
    assert!(removed.status.success(), "{}", String::from_utf8_lossy(&removed.stderr));
    git(&fixture.source, &["submodule", "deinit", "-f", "--all"])?;
    let output = fixture.add(&["--committed", "--submodules", "materialize-pinned"])?;
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(git(&target.join("vendor/child"), &["rev-parse", "HEAD"])?, current);
    assert!(git(&target, &["submodule", "status"])?.starts_with(' '));
    Ok(())
}

#[test]
fn invariant_missing_submodule_objects_fail_before_claiming_the_destination() -> Result {
    let Some(fixture) = Fixture::new(false)? else {
        return Ok(());
    };
    fs::remove_dir_all(fixture.source.join(".git/modules"))?;
    let output = fixture.add(&["--committed", "--submodules", "materialize-pinned"])?;
    assert!(!output.status.success());
    let failure: Failure = serde_json::from_slice(&output.stderr)?;
    assert_eq!(failure.code, "submodule_unavailable");
    assert!(failure.message.contains("vendor/child"));
    assert!(!fixture.target().exists());
    Ok(())
}

#[test]
fn invariant_doctor_reports_the_recorded_submodule_policy() -> Result {
    #[derive(Deserialize)]
    struct Report {
        submodules: cowtree::SubmodulePolicy,
    }
    #[derive(Deserialize)]
    struct Reply {
        value: Report,
    }
    let Some(fixture) = Fixture::new(true)? else {
        return Ok(());
    };
    let output = fixture.add(&["--submodules", "materialize-pinned"])?;
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let output =
        fixture.cowtree(&["doctor", "--json", fixture.target().to_str().ok_or("path")?])?;
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let reply: Reply = serde_json::from_slice(&output.stdout)?;
    assert_eq!(reply.value.submodules, cowtree::SubmodulePolicy::MaterializePinned);
    Ok(())
}

#[test]
fn invariant_nested_children_keep_independent_repositories() -> Result {
    let Some(fixture) = Fixture::new(true)? else {
        return Ok(());
    };
    let grand = fixture.directory.path().join("grand");
    init(&grand)?;
    let child = fixture.source.join("vendor/child");
    git(&child, &["config", "user.name", "Test"])?;
    git(&child, &["config", "user.email", "test@example.invalid"])?;
    git(
        &child,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            grand.to_str().ok_or("path")?,
            "nested/grand",
        ],
    )?;
    git(&child, &["-c", "commit.gpgsign=false", "commit", "-qam", "nested"])?;
    git(&fixture.source, &["commit", "-qam", "nested pin"])?;
    for args in [
        vec!["--submodules", "materialize-pinned"],
        vec!["--committed", "--submodules", "materialize-pinned"],
    ] {
        let output = fixture.add(&args)?;
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let copied = fixture.target().join("vendor/child/nested/grand");
        assert_eq!(fs::read(copied.join("file"))?, b"original\n");
        assert!(copied.join(".git").is_dir());
        let status = git(&fixture.target(), &["submodule", "status", "--recursive"])?;
        assert_eq!(status.lines().count(), 2);
        assert!(status.lines().all(|line| line.starts_with(' ')), "{status}");
        fs::write(copied.join("untracked"), b"keep")?;
        assert!(
            !fixture
                .cowtree(&["remove", fixture.target().to_str().ok_or("path")?])?
                .status
                .success()
        );
        assert!(copied.join("untracked").exists());
        fs::remove_file(copied.join("untracked"))?;
        let output = fixture.cowtree(&["remove", fixture.target().to_str().ok_or("path")?])?;
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(fs::read(child.join("nested/grand/file"))?, b"original\n");
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn invariant_partial_child_failure_retires_only_owned_state() -> Result {
    use std::os::unix::fs::symlink;
    let Some(fixture) = Fixture::new(true)? else {
        return Ok(());
    };
    // A special object-store entry fails after registration and child initialization.
    let modules = fixture.source.join(".git/modules/named-child");
    let sentinel = fixture.directory.path().join("sentinel");
    fs::write(&sentinel, b"caller data")?;
    symlink(&sentinel, modules.join("objects/unsafe-link"))?;
    let target = fixture.target();
    let output = fixture.cowtree(&[
        "add",
        "--json",
        "--committed",
        "--submodules",
        "materialize-pinned",
        "-b",
        "owned",
        target.to_str().ok_or("path")?,
    ])?;
    assert!(!output.status.success());
    assert!(!target.exists());
    assert!(git(&fixture.source, &["branch", "--list", "owned"])?.is_empty());
    assert!(
        !git(&fixture.source, &["worktree", "list", "--porcelain"])?
            .contains(target.to_str().ok_or("path")?)
    );
    assert_eq!(fs::read(&sentinel)?, b"caller data");
    assert!(modules.join("objects/unsafe-link").is_symlink());
    Ok(())
}

#[test]
fn invariant_materialization_does_not_modify_source_module_metadata() -> Result {
    use sha2::{Digest, Sha256};
    fn inventory(root: &Path) -> Result<std::collections::BTreeMap<PathBuf, Vec<u8>>> {
        let mut pending = vec![PathBuf::new()];
        let mut files = std::collections::BTreeMap::new();
        while let Some(relative) = pending.pop() {
            for entry in fs::read_dir(root.join(&relative))? {
                let entry = entry?;
                let relative = relative.join(entry.file_name());
                if entry.file_type()?.is_dir() {
                    pending.push(relative);
                } else {
                    files.insert(relative, Sha256::digest(fs::read(entry.path())?).to_vec());
                }
            }
        }
        Ok(files)
    }
    let Some(fixture) = Fixture::new(true)? else {
        return Ok(());
    };
    let modules = fixture.source.join(".git/modules");
    let before = inventory(&modules)?;
    let output = fixture.add(&["--committed", "--submodules", "materialize-pinned"])?;
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let output = fixture.cowtree(&["remove", fixture.target().to_str().ok_or("path")?])?;
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(inventory(&modules)?, before);
    Ok(())
}

#[test]
fn invariant_external_index_overrides_cannot_redirect_child_writes() -> Result {
    let Some(fixture) = Fixture::new(true)? else {
        return Ok(());
    };
    let outside = fixture.directory.path().join("outside-index");
    fs::write(&outside, b"caller-owned index")?;
    let output = Command::new(env!("CARGO_BIN_EXE_cowtree"))
        .current_dir(&fixture.source)
        .args(["add", "--json", "--committed", "--submodules", "materialize-pinned"])
        .arg(fixture.target())
        .env("GIT_INDEX_FILE", &outside)
        .output()?;
    assert!(!output.status.success());
    assert_eq!(fs::read(&outside)?, b"caller-owned index");
    assert!(!fixture.target().exists());
    Ok(())
}

#[test]
fn invariant_default_uses_gitlinks_from_the_selected_commit() -> Result {
    let Some(fixture) = Fixture::new(false)? else {
        return Ok(());
    };
    let selected = git(&fixture.source, &["rev-parse", "HEAD"])?;
    git(&fixture.source, &["rm", "-q", "vendor/child"])?;
    git(&fixture.source, &["commit", "-qm", "remove child"])?;
    let output = fixture.cowtree(&[
        "add",
        "--json",
        "--committed",
        fixture.target().to_str().ok_or("path")?,
        &selected,
    ])?;
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(fs::read_dir(fixture.target().join("vendor/child"))?.next().is_none());
    assert!(
        git(&fixture.target(), &["submodule", "status"])?.starts_with(&format!("-{}", fixture.pin))
    );
    assert!(!fixture.source.join("vendor/child/.git").exists());
    Ok(())
}

#[test]
fn invariant_existing_worktree_config_is_not_activated_implicitly() -> Result {
    let Some(fixture) = Fixture::new(true)? else {
        return Ok(());
    };
    let dormant = fixture.source.join(".git/config.worktree");
    fs::write(&dormant, b"[core]\nworktree = /must-not-activate\n")?;
    let output = fixture.add(&["--committed", "--submodules", "materialize-pinned"])?;
    assert!(!output.status.success());
    assert!(!fixture.target().exists());
    assert_eq!(fs::read(&dormant)?, b"[core]\nworktree = /must-not-activate\n");
    assert_eq!(
        git(&fixture.source, &["rev-parse", "--show-toplevel"])?,
        fixture.source.canonicalize()?.to_str().ok_or("path")?
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn invariant_checkout_hooks_cannot_redirect_child_initialization() -> Result {
    use std::os::unix::fs::PermissionsExt;
    let Some(fixture) = Fixture::new(true)? else {
        return Ok(());
    };
    let metadata = fixture.source.join(".git/modules/named-child");
    let config = fs::read(metadata.join("config"))?;
    let hook = fixture.source.join(".git/hooks/post-checkout");
    fs::write(&hook, b"#!/bin/sh\nprintf 'gitdir: %s/modules/named-child\\n' \"$(git rev-parse --path-format=absolute --git-common-dir)\" > vendor/child/.git\n")?;
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755))?;
    let output = fixture.add(&["--committed", "--submodules", "materialize-pinned"])?;
    assert!(!output.status.success());
    assert!(!fixture.target().exists());
    assert_eq!(fs::read(metadata.join("config"))?, config);
    assert_eq!(git(&fixture.source.join("vendor/child"), &["rev-parse", "HEAD"])?, fixture.pin);
    Ok(())
}

#[test]
fn invariant_incomplete_object_sources_are_never_fetched_or_shared() -> Result {
    let Some(fixture) = Fixture::new(true)? else {
        return Ok(());
    };
    let child = fixture.source.join("vendor/child");
    git(&child, &["config", "remote.origin.promisor", "true"])?;
    let output = fixture.add(&["--committed", "--submodules", "materialize-pinned"])?;
    assert!(!output.status.success());
    let failure: Failure = serde_json::from_slice(&output.stderr)?;
    assert_eq!(failure.code, "submodule_unavailable");
    assert!(!fixture.target().exists());
    git(&child, &["config", "--unset", "remote.origin.promisor"])?;
    let alternate = fixture.source.join(".git/modules/named-child/objects/info/alternates");
    fs::write(&alternate, b"/unavailable/objects\n")?;
    let output = fixture.add(&["--committed", "--submodules", "materialize-pinned"])?;
    assert!(!output.status.success());
    assert_eq!(fs::read(&alternate)?, b"/unavailable/objects\n");
    assert!(!fixture.target().exists());
    Ok(())
}
