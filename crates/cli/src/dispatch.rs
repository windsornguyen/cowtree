// Copyright (c) 2026 Windsor Nguyen

//! Dispatch native commands only after parsing has selected one operation.

use crate::{
    arguments::{Arguments, Operation},
    output,
};
use clap::{CommandFactory, Parser, error::ErrorKind};
use cowtree::{PreparedAdd, WorktreeError, inspect_path, list_worktrees, remove_worktree};
use std::{
    ffi::OsString,
    io::{self, Write},
    path::Path,
    process::ExitCode,
};

pub fn run() -> io::Result<ExitCode> {
    let arguments: Vec<OsString> = std::env::args_os().collect();
    let json = arguments
        .iter()
        .take_while(|argument| *argument != "--")
        .any(|argument| argument == "--json")
        || arguments.get(1).is_some_and(|argument| argument == "workspace");
    let options = match Arguments::try_parse_from(arguments) {
        Ok(options) => options,
        Err(error)
            if matches!(error.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) =>
        {
            error.print()?;
            return Ok(ExitCode::SUCCESS);
        }
        Err(error) => {
            output::failure("invalid_arguments", &error.to_string(), json)?;
            return Ok(ExitCode::from(2));
        }
    };
    match execute(options) {
        Ok(code) => Ok(ExitCode::from(code)),
        Err(Failure::Operation(error)) => {
            output::failure(error.code(), &error.to_string(), json)?;
            Ok(if error.code() == "cancelled" { ExitCode::from(130) } else { ExitCode::FAILURE })
        }
        Err(Failure::Output(error)) => {
            output::failure("output_failed", &error.to_string(), json)?;
            Ok(ExitCode::FAILURE)
        }
        Err(Failure::Signal(error)) => {
            output::failure("command_failed", &error.to_string(), json)?;
            Ok(ExitCode::FAILURE)
        }
    }
}

enum Failure {
    Operation(WorktreeError),
    Output(io::Error),
    Signal(ctrlc::Error),
}
impl From<WorktreeError> for Failure {
    fn from(error: WorktreeError) -> Self {
        Self::Operation(error)
    }
}
impl From<io::Error> for Failure {
    fn from(error: io::Error) -> Self {
        Self::Output(error)
    }
}

fn execute(options: Arguments) -> Result<u8, Failure> {
    if options.version {
        if options.command.is_some() {
            output::failure(
                "invalid_arguments",
                "--version cannot be combined with an operation",
                options.json,
            )?;
            return Ok(2);
        }
        version(options.json)?;
        return Ok(0);
    }
    match options.command {
        #[cfg(unix)]
        Some(Operation::Workspace(options)) => return Ok(crate::workspace::run(options)?),
        #[cfg(unix)]
        Some(Operation::Supervise { timeout, command }) => {
            return cowtree_process::run(&command, timeout)
                .map_err(|error| Failure::Output(io::Error::other(error)));
        }
        Some(Operation::Add(arguments)) => {
            let request = arguments.request();
            let prepared = PreparedAdd::new(&request)?;
            let cancellation = request.cancellation.clone();
            ctrlc::set_handler(move || cancellation.cancel()).map_err(Failure::Signal)?;
            let tree = prepared.run()?;
            if options.json {
                output::success("add", tree)?;
            } else {
                writeln!(io::stdout().lock(), "{}", tree.path.display())?;
            }
        }
        Some(Operation::List { source }) => {
            let trees = list_worktrees(source.as_deref())?;
            output::worktrees(&trees, options.json)?;
        }
        Some(Operation::Remove { path, force }) => {
            remove_worktree(&path, None, force)?;
            if options.json {
                output::success("remove", ())?;
            }
        }
        Some(Operation::Doctor { directory }) => {
            let report = inspect_path(directory.as_deref().unwrap_or(Path::new(".")))?;
            let code = if report.supported() { 0 } else { 1 };
            if options.json {
                output::success("doctor", report)?;
            } else if let Some(tool) = report.clone_tool {
                writeln!(
                    io::stdout().lock(),
                    "{}: {} via {tool}",
                    report.path.display(),
                    report.filesystem
                )?;
            } else {
                writeln!(
                    io::stdout().lock(),
                    "{}: unsupported: {:?}",
                    report.path.display(),
                    report.reason
                )?;
            }
            return Ok(code);
        }
        Some(Operation::Help) | None => Arguments::command().print_help()?,
    }
    Ok(0)
}

fn version(json: bool) -> io::Result<()> {
    let version = crate::version::Version::current();
    if json {
        output::write_json(&version, &mut io::stdout().lock())
    } else {
        writeln!(
            io::stdout().lock(),
            "cowtree {} (revision {})",
            version.version,
            version.revision.unwrap_or("unknown")
        )
    }
}
