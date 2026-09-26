// Copyright (c) 2026 Windsor Nguyen

//! Bound tool execution while draining both output streams concurrently.

use std::{
    io::Read,
    process::{Command, Output, Stdio},
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use wait_timeout::ChildExt;

pub struct Execution {
    /// Captured output and the reaped child's actual status.
    pub output: Output,
    /// The caller's wall-time budget required termination.
    pub timed_out: bool,
}

pub fn output(command: &mut Command, timeout: Duration) -> Result<Execution> {
    let mut child = command.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let result = thread::scope(|scope| {
        let stdout = child.stdout.take().context("configured stdout pipe is missing")?;
        let stderr = child.stderr.take().context("configured stderr pipe is missing")?;
        let stdout = scope.spawn(move || read(stdout));
        let stderr = scope.spawn(move || read(stderr));
        let (status, timed_out) = match child.wait_timeout(timeout) {
            Ok(Some(status)) => (status, false),
            Ok(None) => {
                child.kill()?;
                (child.wait()?, true)
            }
            Err(error) => {
                child.kill()?;
                child.wait()?;
                return Err(error.into());
            }
        };
        Ok(Execution {
            output: Output {
                status,
                stdout: stdout.join().map_err(|_| anyhow::anyhow!("stdout reader panicked"))??,
                stderr: stderr.join().map_err(|_| anyhow::anyhow!("stderr reader panicked"))??,
            },
            timed_out,
        })
    });
    if result.is_err() && child.try_wait()?.is_none() {
        child.kill()?;
        child.wait()?;
    }
    result
}

fn read(mut stream: impl Read) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes)?;
    Ok(bytes)
}

pub fn checked(command: &mut Command, timeout: Duration) -> Result<String> {
    let execution = output(command, timeout)?;
    if execution.timed_out {
        bail!("tool exceeded {} seconds", timeout.as_secs());
    }
    let output = execution.output;
    if !output.status.success() {
        bail!("tool exited {}: {}", output.status, String::from_utf8_lossy(&output.stderr));
    }
    String::from_utf8(output.stdout).context("tool returned non-UTF-8 output")
}
