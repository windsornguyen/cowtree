// Copyright (c) 2026 Windsor Nguyen

//! Bound validation time and logs even after the workspace coordinator exits.
//!
//! Stdin carries an inherited operation-lock descriptor. The check receives a
//! duplicate of that lock and a null stdin. Only its new process group is signaled.

use std::{
    ffi::OsString,
    os::{
        fd::AsFd,
        unix::process::{CommandExt, ExitStatusExt},
    },
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

use command_fds::CommandFdExt;
use rustix::process::{
    Pid, Signal, WaitId, WaitIdOptions, kill_process_group, test_kill_process_group, waitid,
};

use crate::{Error, Result};

pub fn run(command: &[OsString], timeout_seconds: u64) -> Result<u8> {
    let Some((program, arguments)) = command.split_first() else {
        return Err(Error::InvalidRequest);
    };
    if timeout_seconds == 0 {
        return Err(Error::InvalidRequest);
    }
    let lock = std::io::stdin().as_fd().try_clone_to_owned()?;
    let mut process = Command::new(program);
    process.args(arguments).stdin(Stdio::null()).process_group(0).preserved_fds(vec![lock]);
    let mut child = process.spawn()?;
    let outcome = monitor(&child, Duration::from_secs(timeout_seconds));
    // Another reaper has invalidated our PID ownership witness. Do not signal it.
    if matches!(outcome, Err(Error::Wait(rustix::io::Errno::CHILD))) {
        return Err(Error::Wait(rustix::io::Errno::CHILD));
    }
    let status = match terminate(&mut child) {
        Ok(status) => status,
        Err(Error::Signal(rustix::io::Errno::PERM)) if matches!(outcome, Ok(Outcome::Exited)) => {
            // Darwin can refuse a signal to an exited leader. Prove the group is gone
            // after reaping. Never signal after reaping, when its ID may be reused.
            let group = Pid::from_child(&child);
            let status = child.wait()?;
            if !matches!(test_kill_process_group(group), Err(rustix::io::Errno::SRCH)) {
                return Err(Error::Signal(rustix::io::Errno::PERM));
            }
            status
        }
        Err(cleanup) => {
            return Err(match outcome {
                Ok(_) => cleanup,
                Err(original) => {
                    Error::Cleanup { original: Box::new(original), cleanup: Box::new(cleanup) }
                }
            });
        }
    };
    match outcome {
        Ok(Outcome::Exited) => exit_code(status),
        Ok(Outcome::Deadline) => Ok(124),
        Ok(Outcome::LogLimit) => Ok(125),
        Err(error) => Err(error),
    }
}

enum Outcome {
    Exited,
    Deadline,
    LogLimit,
}

fn monitor(child: &Child, timeout: Duration) -> Result<Outcome> {
    let started = Instant::now();
    loop {
        // Leave the leader unreaped until group cleanup, preventing PGID reuse.
        let options = WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT;
        match waitid(WaitId::Pid(Pid::from_child(child)), options) {
            Ok(Some(_)) => return Ok(Outcome::Exited),
            Ok(None) | Err(rustix::io::Errno::INTR) => {}
            Err(error) => return Err(Error::Wait(error)),
        }
        let output = rustix::fs::fstat(std::io::stdout()).map_err(std::io::Error::from)?;
        if rustix::fs::FileType::from_raw_mode(output.st_mode) == rustix::fs::FileType::RegularFile
            && output.st_size > 128 * 1024 * 1024
        {
            return Ok(Outcome::LogLimit);
        }
        if started.elapsed() >= timeout {
            return Ok(Outcome::Deadline);
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn terminate(child: &mut Child) -> Result<ExitStatus> {
    match kill_process_group(Pid::from_child(child), Signal::KILL) {
        Ok(()) | Err(rustix::io::Errno::SRCH) => {}
        Err(error) => return Err(Error::Signal(error)),
    }
    Ok(child.wait()?)
}

fn exit_code(status: ExitStatus) -> Result<u8> {
    if let Some(code) = status.code() {
        return u8::try_from(code).map_err(|_| Error::ExitStatus);
    }
    let signal = status.signal().ok_or(Error::ExitStatus)?;
    u8::try_from(128 + signal).map_err(|_| Error::ExitStatus)
}
