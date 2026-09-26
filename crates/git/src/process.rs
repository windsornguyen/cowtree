// Copyright (c) 2026 Windsor Nguyen

//! Drain Git output while supplying bounded input.

use crate::{Client, Error, Result};
use std::{
    io::{self, Write},
    process::{Command, Output, Stdio},
};

impl Client {
    pub fn output(&self, command: &mut Command) -> Result<Output> {
        command.output().map_err(|error| Error::io(&self.root, error))
    }

    pub fn capture(&self, command: &mut Command) -> Result<Vec<u8>> {
        checked(self.output(command)?)
    }

    pub fn text(&self, command: &mut Command) -> Result<String> {
        let bytes = crate::client::trim_newline(self.capture(command)?);
        String::from_utf8(bytes).map_err(|source| Error::Encoding { source })
    }

    pub fn input(&self, command: &mut Command, input: &[u8]) -> Result<Vec<u8>> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| Error::io(&self.root, error))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::io(&self.root, io::Error::other("missing piped Git stdin")))?;
        std::thread::scope(|scope| {
            let writer = scope.spawn(move || stdin.write_all(input));
            let output = child.wait_with_output().map_err(|error| Error::io(&self.root, error));
            let written = writer.join().map_err(|_| {
                Error::io(&self.root, io::Error::other("Git input writer panicked"))
            })?;
            let output = checked(output?)?;
            written.map_err(|error| Error::io(&self.root, error))?;
            Ok(output)
        })
    }
}

pub fn checked(output: Output) -> Result<Vec<u8>> {
    if output.status.success() {
        return Ok(output.stdout);
    }
    Err(Error::Command {
        status: output.status,
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}
