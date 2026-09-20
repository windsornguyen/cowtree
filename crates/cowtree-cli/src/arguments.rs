// Copyright (c) 2026 Windsor Nguyen

//! Parse the supported standalone contract without forwarding unknown Git flags.

use clap::{
    Args, Parser, Subcommand,
    builder::{NonEmptyStringValueParser, OsStringValueParser, TypedValueParser},
};
use cowtree::{AddRequest, Branch, Lock, SourceMode};
use std::{ffi::OsString, path::PathBuf};

#[derive(Parser)]
#[command(name = "cowtree", about = "Copy-on-write Git worktrees", disable_help_subcommand = true)]
pub(crate) struct Arguments {
    #[arg(long, global = true)]
    pub(crate) json: bool,
    #[arg(long)]
    pub(crate) version: bool,
    #[command(subcommand)]
    pub(crate) command: Option<Operation>,
}

#[derive(Subcommand)]
pub(crate) enum Operation {
    Add(Add),
    List {
        source: Option<PathBuf>,
    },
    Remove {
        path: PathBuf,
        #[arg(long)]
        force: bool,
    },
    Doctor {
        directory: Option<PathBuf>,
    },
    Help,
}

#[derive(Args)]
#[command(group(clap::ArgGroup::new("branch_mode").args(["new_branch", "existing_branch", "detach"])))]
pub(crate) struct Add {
    #[arg(short = 'b', value_parser = nonempty())]
    new_branch: Option<OsString>,
    #[arg(long = "branch", value_parser = nonempty())]
    existing_branch: Option<OsString>,
    #[arg(long, short = 'd')]
    detach: bool,
    #[arg(long)]
    committed: bool,
    #[arg(long)]
    lock: bool,
    #[arg(long, requires = "lock", value_parser = NonEmptyStringValueParser::new())]
    reason: Option<String>,
    path: PathBuf,
    #[arg(value_parser = nonempty())]
    revision: Option<OsString>,
}

fn nonempty() -> impl TypedValueParser<Value = OsString> {
    OsStringValueParser::new().try_map(|value| {
        if value.is_empty() {
            return Err("value must be nonempty");
        }
        Ok(value)
    })
}

impl Add {
    pub(crate) fn request(self) -> AddRequest {
        let mut request = AddRequest::new(self.path);
        request.branch = match (self.new_branch, self.existing_branch) {
            (Some(branch), _) => Branch::New(branch),
            (_, Some(branch)) => Branch::Existing(branch),
            _ => Branch::Detached,
        };
        if let Some(revision) = self.revision {
            request.revision = revision;
        }
        request.source_mode =
            if self.committed { SourceMode::Committed } else { SourceMode::Checkout };
        request.lock = if self.lock { Lock::Retain { reason: self.reason } } else { Lock::Release };
        request
    }
}
