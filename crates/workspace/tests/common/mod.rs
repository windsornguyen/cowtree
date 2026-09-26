// Copyright (c) 2026 Windsor Nguyen

//! Real filesystem and Git fixture shared by managed workflow integration tests.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use cowtree_workspace::{CreateRequest, Policy, Workspace};

pub type TestResult = Result<(), Box<dyn std::error::Error>>;

pub struct Fixture {
    pub directory: tempfile::TempDir,
    pub source: PathBuf,
    pub workspace: Workspace,
}

pub fn git(path: &Path, arguments: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let output = Command::new("git").arg("-C").arg(path).args(arguments).output()?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    Ok(String::from_utf8(output.stdout)?)
}

pub fn fixture() -> Result<Option<Fixture>, Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let supported = cowtree::inspect_path(directory.path())?.supported();
    if std::env::var_os("COWTREE_EXPECT_SUPPORTED").is_some_and(|value| value == "1") {
        assert!(supported);
    }
    if !supported {
        eprintln!("skip native filesystem");
        return Ok(None);
    }
    let source = directory.path().join("source");
    fs::create_dir(&source)?;
    fs::create_dir(source.join("target"))?;
    fs::write(source.join("file"), b"original\n")?;
    fs::write(source.join(".gitignore"), b"target/\n")?;
    fs::write(source.join("target/cache"), b"warm cache")?;
    for arguments in [
        vec!["init", "-q"],
        vec!["config", "user.name", "Cowtree test"],
        vec!["config", "user.email", "test@example.invalid"],
        vec!["config", "commit.gpgsign", "false"],
        vec!["add", "."],
        vec!["commit", "-qm", "fixture"],
    ] {
        git(&source, &arguments)?;
    }
    let workspace = Workspace::create(&CreateRequest {
        root: directory.path().join("workspace"),
        source: source.clone(),
        program: std::env::current_exe()?,
        policy: Policy { derived: vec!["target".into()], ..Policy::default() },
    })?;
    Ok(Some(Fixture { directory, source, workspace }))
}
