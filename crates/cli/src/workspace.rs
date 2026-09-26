// Copyright (c) 2026 Windsor Nguyen

//! Run one native workspace operation and emit its typed JSON result.

use std::{collections::BTreeMap, fs, io, path::Path};

use cowtree_metadata::{BatchCandidate, RequestId, ResourcePath};
use cowtree_workspace::Retention;
use cowtree_workspace::{
    CheckRequest, Choice, CreateRequest, DropPolicy, Error, Hardlinks, Policy, SubmodulePolicy,
    Workspace,
};
use serde::{Serialize, de::DeserializeOwned};

use crate::{
    output,
    workspace_arguments::{Action, Options},
};

pub(crate) fn run(options: Options) -> io::Result<u8> {
    match execute(options) {
        Ok(()) => Ok(0),
        Err(Failure::Output(error)) => Err(error),
        Err(Failure::Domain(error)) => {
            if let Error::Authority(authority) = &error {
                #[derive(Serialize)]
                struct AuthorityFailure {
                    /// Discriminant shared by all CLI errors.
                    status: &'static str,
                    /// Stable authority reason, retry instruction, and context.
                    #[serde(flatten)]
                    error: cowtree_metadata::WireError,
                }
                output::write_json(
                    &AuthorityFailure { status: "error", error: authority.wire() },
                    &mut io::stderr().lock(),
                )?;
                return Ok(1);
            }
            output::failure(error.code(), &error.to_string(), true)?;
            Ok(if error.code() == "invalid_arguments" { 2 } else { 1 })
        }
    }
}

enum Failure {
    Domain(Error),
    Output(io::Error),
}
impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        Self::Domain(error)
    }
}
impl From<io::Error> for Failure {
    fn from(error: io::Error) -> Self {
        Self::Output(error)
    }
}
impl From<cowtree_metadata::Error> for Failure {
    fn from(error: cowtree_metadata::Error) -> Self {
        Self::Domain(error.into())
    }
}

fn execute(options: Options) -> Result<(), Failure> {
    match options.action {
        Action::Init { source, derived, ephemeral, derived_hardlinks, submodules } => {
            let policy = Policy {
                derived,
                ephemeral,
                derived_hardlinks: match derived_hardlinks {
                    crate::workspace_arguments::Hardlinks::Reject => Hardlinks::Reject,
                    crate::workspace_arguments::Hardlinks::Clone => Hardlinks::Clone,
                },
                submodules: match submodules {
                    crate::workspace_arguments::Submodules::Reject => SubmodulePolicy::Reject,
                    crate::workspace_arguments::Submodules::MaterializePinned => {
                        SubmodulePolicy::MaterializePinned
                    }
                },
                ..Policy::default()
            };
            let workspace = Workspace::create(&CreateRequest {
                root: options.root,
                source,
                program: std::env::current_exe()?,
                policy,
            })?;
            output::success("init", workspace.config)?;
        }
        Action::ImportStatus => {
            output::success("import-status", Workspace::import_status(&options.root)?)?
        }
        Action::Recover if !options.root.exists() => {
            output::success("recover", Workspace::recover_initialization(&options.root)?)?
        }
        Action::Version => {
            #[derive(Serialize)]
            struct Versions {
                /// Native CLI build identity.
                cli: crate::version::Version,
                /// Linked metadata authority build identity.
                backend: cowtree_metadata::BuildVersion,
            }
            output::success(
                "version",
                Versions {
                    cli: crate::version::Version::current(),
                    backend: cowtree_metadata::BuildVersion::current(),
                },
            )?;
        }
        action => dispatch(&Workspace::open(&options.root)?, action)?,
    }
    Ok(())
}

fn dispatch(workspace: &Workspace, action: Action) -> Result<(), Failure> {
    match action {
        Action::List => output::success("list", workspace.list()?)?,
        Action::Fork { path, node } => output::success("fork", workspace.fork(&path, node)?)?,
        Action::Acquire(selection) => output::success(
            "acquire",
            workspace.acquire(selection.identity, &paths(selection.paths)?)?,
        )?,
        Action::Sync(identity) => output::success("sync", workspace.sync(identity.identity)?)?,
        Action::Discard(selection) => output::success(
            "discard",
            workspace.discard(selection.identity, &paths(selection.paths)?)?,
        )?,
        Action::Seal(identity) => {
            output::success("seal", workspace.seal(identity.identity, Retention::Manual)?)?
        }
        Action::Retain(checkpoint) => {
            workspace.retain(&checkpoint.node)?;
            output::success("retain", ())?;
        }
        Action::Release(checkpoint) => {
            workspace.release(&checkpoint.node)?;
            output::success("release", ())?;
        }
        Action::Capture(identity) => {
            output::success("capture", workspace.capture(identity.identity)?)?
        }
        Action::Prepare(identity) => {
            output::success("prepare", workspace.prepare(identity.identity)?)?
        }
        Action::Check { identity, timeout, command } => {
            check(workspace, identity, timeout, command)?
        }
        Action::Commit(identity) => {
            let candidate = workspace.pending_candidate(identity.identity)?;
            output::success("commit", workspace.commit(&candidate)?)?;
        }
        Action::Result { identity, sequence } => {
            output::success("result", workspace.result(RequestId { leaf: identity, sequence })?)?
        }
        Action::Abort(identity) => {
            workspace.abort(identity.identity)?;
            output::success("abort", ())?;
        }
        Action::Drop { identity, force } => retire(workspace, identity, force)?,
        Action::Collect => output::success("collect", workspace.collect()?)?,
        Action::Recover => output::success("recover", workspace.recover()?)?,
        Action::Log => output::success("log", workspace.log()?)?,
        Action::Resolve { identity, choices } => {
            let choices: BTreeMap<ResourcePath, Choice> = read(&choices)?;
            output::success("resolve", workspace.resolve(identity, &choices)?)?;
        }
        Action::PrepareBatch { identities } => {
            output::success("prepare-batch", workspace.prepare_batch(&identities)?)?
        }
        Action::CheckBatch { candidate, timeout, command } => {
            let candidate: BatchCandidate = read(&candidate)?;
            output::success(
                "check-batch",
                workspace.check_batch(&candidate, command, timeout, std::env::current_exe()?)?,
            )?;
        }
        Action::CommitBatch { candidate } => {
            let candidate: BatchCandidate = read(&candidate)?;
            output::success("commit-batch", workspace.commit_batch(&candidate)?)?;
        }
        Action::Init { .. } | Action::ImportStatus | Action::Version => {
            return Err(io::Error::other("initialization dispatched as an open workspace").into());
        }
    }
    Ok(())
}

fn paths(values: Vec<String>) -> Result<Vec<ResourcePath>, Failure> {
    values.into_iter().map(|value| ResourcePath::parse(value).map_err(Failure::from)).collect()
}

fn read<T: DeserializeOwned>(path: &Path) -> Result<T, Failure> {
    let bytes = fs::read(path).map_err(|source| Error::Io { path: path.into(), source })?;
    serde_json::from_slice(&bytes)
        .map_err(|source| Error::Record { path: path.into(), source }.into())
}

fn retire(
    workspace: &Workspace,
    identity: cowtree_metadata::LeafId,
    force: bool,
) -> Result<(), Failure> {
    let policy = if force { DropPolicy::DiscardPrivate } else { DropPolicy::RequireClean };
    workspace.drop_leaf(identity, policy)?;
    output::success("drop", ())?;
    Ok(())
}

fn check(
    workspace: &Workspace,
    identity: cowtree_metadata::LeafId,
    timeout_seconds: u64,
    command: Vec<String>,
) -> Result<(), Failure> {
    let request =
        CheckRequest { supervisor: std::env::current_exe()?, identity, command, timeout_seconds };
    output::success("check", workspace.check(&request)?)?;
    Ok(())
}
