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

/// Hide the machine's system and global Git configuration from a command. Git for Windows sets
/// `core.autocrlf=true` system-wide, which would rewrite the checked-out bytes the tests compare,
/// and Cowtree's own Git calls inherit the same environment.
fn without_machine_config(command: &mut Command) -> &mut Command {
    command.env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_CONFIG_GLOBAL", empty_global_config())
}

/// An empty file to stand in for the global configuration. `NUL` is not a readable path for
/// every Git for Windows build, so the tests name a real file on every platform.
fn empty_global_config() -> &'static Path {
    static EMPTY: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    EMPTY.get_or_init(|| {
        let path =
            std::env::temp_dir().join(format!("cowtree-empty-gitconfig-{}", std::process::id()));
        fs::write(&path, b"").expect("write the empty global Git configuration");
        path
    })
}

pub fn git(root: &Path, args: &[&str]) -> Result<String> {
    let output =
        without_machine_config(Command::new("git").arg("-C").arg(root).args(args)).output()?;
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
        let mut command = Command::new(env!("CARGO_BIN_EXE_cowtree"));
        command.current_dir(&self.source).args(args);
        Ok(without_machine_config(&mut command).output()?)
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
