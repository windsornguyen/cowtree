// Copyright (c) 2026 Windsor Nguyen

//! Real local repositories for standalone submodule tests.

use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

pub type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

pub struct Fixture {
    pub directory: tempfile::TempDir,
    pub source: PathBuf,
    pub pin: String,
}

pub fn git(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", if cfg!(windows) { "NUL" } else { "/dev/null" })
        .output()?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    Ok(String::from_utf8(output.stdout)?.trim_end().into())
}

pub fn init(root: &Path) -> Result {
    fs::create_dir(root)?;
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.name", "Test"],
        vec!["config", "user.email", "test@example.invalid"],
        vec!["config", "commit.gpgsign", "false"],
    ] {
        git(root, &args)?;
    }
    fs::write(root.join("file"), b"original\n")?;
    git(root, &["add", "."])?;
    git(root, &["commit", "-qm", "fixture"])?;
    Ok(())
}

impl Fixture {
    pub fn new(initialized: bool) -> Result<Option<Self>> {
        let directory = tempfile::tempdir()?;
        let supported = cowtree::inspect_path(directory.path())?.supported();
        if std::env::var_os("COWTREE_EXPECT_SUPPORTED").is_some_and(|value| value == "1") {
            assert!(supported);
        }
        if !supported {
            return Ok(None);
        }
        let source = directory.path().join("source");
        let child = directory.path().join("child");
        init(&source)?;
        init(&child)?;
        git(
            &source,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                "--name",
                "named-child",
                child.to_str().ok_or("path")?,
                "vendor/child",
            ],
        )?;
        git(&source, &["commit", "-qam", "pin"])?;
        let pin = git(&child, &["rev-parse", "HEAD"])?;
        if !initialized {
            git(&source, &["submodule", "deinit", "-f", "--all"])?;
        }
        Ok(Some(Self { directory, source, pin }))
    }

    pub fn target(&self) -> PathBuf {
        self.directory.path().join("target")
    }
    pub fn cowtree(&self, args: &[&str]) -> Result<Output> {
        Ok(Command::new(env!("CARGO_BIN_EXE_cowtree"))
            .current_dir(&self.source)
            .args(args)
            .output()?)
    }
    pub fn add(&self, args: &[&str]) -> Result<Output> {
        let target = self.target();
        let mut argv = vec!["add", "--json", "--detach"];
        argv.extend_from_slice(args);
        argv.push(target.to_str().ok_or("path")?);
        self.cowtree(&argv)
    }
}

#[derive(Deserialize)]
pub struct Failure {
    pub code: String,
    pub message: String,
}
