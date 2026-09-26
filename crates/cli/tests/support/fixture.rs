// Copyright (c) 2026 Windsor Nguyen

//! Construct private repositories and decode the shipped CLI's typed responses.

use cowtree_workspace::{Config, Leaf, Pending, Validation};
use serde::{Deserialize, de::DeserializeOwned};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

pub struct Fixture {
    /// Owns every repository, worktree, and journal created by this test.
    pub directory: tempfile::TempDir,
    /// Parent checkout whose source must remain unchanged.
    pub source: PathBuf,
    /// Native managed store.
    pub store: PathBuf,
}

#[derive(Deserialize)]
struct Reply<T> {
    /// Stable CLI success discriminant.
    status: String,
    /// Result decoded into the operation's domain type.
    value: T,
}

impl Fixture {
    pub fn new() -> TestResult<Option<Self>> {
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
        fs::write(source.join("other"), b"base\n")?;
        fs::write(source.join(".gitignore"), b"target/\n")?;
        fs::write(source.join("target/cache"), b"initial cache")?;
        for argv in [
            vec!["init", "-q"],
            vec!["config", "user.name", "Test"],
            vec!["config", "user.email", "test@example.invalid"],
            vec!["config", "commit.gpgsign", "false"],
            vec!["add", "."],
            vec!["commit", "-qm", "fixture"],
        ] {
            let output = Command::new("git").arg("-C").arg(&source).args(argv).output()?;
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        }
        let store = directory.path().join("store");
        let fixture = Self { directory, source, store };
        let _: Config =
            fixture.call(&["init", "--source", text(&fixture.source)?, "--derived", "target"])?;
        Ok(Some(fixture))
    }

    pub fn command(&self, argv: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cowtree"));
        command.args(["workspace", "--root"]).arg(&self.store).args(argv);
        command
    }

    pub fn call<T: DeserializeOwned>(&self, argv: &[&str]) -> TestResult<T> {
        decode(self.command(argv).output()?)
    }

    pub fn fork(&self, name: &str) -> TestResult<Leaf> {
        self.call(&["fork", text(&self.directory.path().join(name))?])
    }

    pub fn capture(&self, leaf: &Leaf, file: &str, bytes: &[u8]) -> TestResult {
        fs::write(leaf.path.join(file), bytes)?;
        let pending: Option<Pending> = self.call(&["capture", &leaf.id.get().to_string()])?;
        assert!(pending.is_some());
        Ok(())
    }

    pub fn check(&self, leaf: &Leaf, script: &str) -> TestResult<Validation> {
        self.call(&["check", &leaf.id.get().to_string(), "--", "/bin/sh", "-c", script])
    }
}

pub fn text(path: &Path) -> TestResult<&str> {
    path.to_str().ok_or_else(|| "non-UTF-8 test path".into())
}

pub fn decode<T: DeserializeOwned>(output: Output) -> TestResult<T> {
    if !output.status.success() {
        return Err(
            format!("{}: {}", output.status, String::from_utf8_lossy(&output.stderr)).into()
        );
    }
    let reply: Reply<T> = serde_json::from_slice(&output.stdout)?;
    assert_eq!(reply.status, "ok");
    Ok(reply.value)
}
