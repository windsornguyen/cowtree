// Copyright (c) 2026 Windsor Nguyen

//! Exercise real Git pipes and inherited ownership.

use cowtree_git::{Client, Error};
use std::io;

#[test]
fn streamed_queries_drain_output_before_input_finishes() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let git = Client::at(directory.path());
    git.capture(git.command()?.args(["init", "-q"]))?;
    let bytes = vec![b'x'; 512];
    let oid = String::from_utf8(
        git.input(git.command()?.args(["hash-object", "-w", "--stdin"]), &bytes)?,
    )?;
    let query = oid.repeat(4096);
    let output = git.input(git.command()?.args(["cat-file", "--batch"]), query.as_bytes())?;
    let mut record = format!("{} blob {}\n", oid.trim_end(), bytes.len()).into_bytes();
    record.extend_from_slice(&bytes);
    record.push(b'\n');
    assert_eq!(output, record.repeat(4096));
    Ok(())
}

#[test]
fn command_failures_retain_their_exit_status_and_diagnostic()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let git = Client::at(directory.path());
    let error = git
        .capture(git.command()?.arg("--cowtree-invalid-option"))
        .err()
        .ok_or("invalid command succeeded")?;
    assert!(
        matches!(error, Error::Command { status, stderr } if !status.success() && !stderr.is_empty())
    );
    let error = git
        .capture(&mut std::process::Command::new(directory.path().join("missing")))
        .err()
        .ok_or("missing executable started")?;
    assert!(matches!(error, Error::Io { source, .. } if source.kind() == io::ErrorKind::NotFound));
    Ok(())
}

#[cfg(unix)]
#[test]
fn inherited_lock_outlives_its_parent_handle() -> Result<(), Box<dyn std::error::Error>> {
    use std::{
        fs::File,
        io::{BufRead, BufReader, Write},
        process::Stdio,
    };
    let directory = tempfile::tempdir()?;
    let git = Client::at(directory.path());
    git.capture(git.command()?.args(["init", "-q"]))?;
    let common = git.directory()?;
    let held = git.locked_at(&common)?;
    let mut child = held
        .command()?
        .args(["-c", "alias.hold=!printf 'ready\\n'; read value", "hold"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let mut ready = String::new();
    BufReader::new(child.stdout.take().ok_or("missing stdout")?).read_line(&mut ready)?;
    drop(held);
    let contender = File::options().read(true).write(true).open(common.join("cowtree.lock"))?;
    let blocked = matches!(fs4::FileExt::try_lock(&contender), Err(fs4::TryLockError::WouldBlock));
    child.stdin.take().ok_or("missing stdin")?.write_all(b"done\n")?;
    let status = child.wait()?;
    assert_eq!(ready, "ready\n");
    assert!(blocked);
    assert!(status.success());
    fs4::FileExt::try_lock(&contender)?;
    Ok(())
}
