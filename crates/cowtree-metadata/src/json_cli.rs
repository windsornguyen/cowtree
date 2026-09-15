// Copyright (c) 2026 Windsor Nguyen

//! Line-delimited JSON interface to the local metadata authority.

use cowtree_metadata::{
    Candidate, Entry, Grant, LeafId, LeafView, Limits, Maintenance, Proposal, ProposalInput,
    Receipt, RequestId, ResourcePath, Snapshot, Store, Version, objects::ObjectId,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    io::{self, BufRead, Write},
    path::Path,
};

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
    Init {
        #[serde(default)]
        limits: Limits,
    },
    CreateLeaf,
    Tip,
    Snapshot {
        version: Version,
    },
    Read {
        version: Version,
        path: ResourcePath,
    },
    Acquire {
        leaf: LeafId,
        paths: BTreeSet<ResourcePath>,
    },
    Activate {
        grant: Grant,
    },
    Stage {
        leaf: LeafId,
        data: Vec<u8>,
    },
    Edit {
        grant: Grant,
        value: Option<Entry>,
    },
    View {
        leaf: LeafId,
    },
    Release {
        grant: Grant,
    },
    Revoke {
        path: ResourcePath,
    },
    Discard {
        leaf: LeafId,
        path: ResourcePath,
    },
    DiscardUpload {
        leaf: LeafId,
        object: ObjectId,
    },
    DropLeaf {
        leaf: LeafId,
    },
    Propose {
        input: ProposalInput,
    },
    Prepare {
        request: RequestId,
    },
    Commit {
        candidate: Candidate,
    },
    Result {
        request: RequestId,
    },
    Abort {
        request: RequestId,
    },
    Retain {
        version: Version,
    },
    ReleaseRetention {
        version: Version,
    },
    Maintain,
}
#[derive(Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
enum Output {
    Done,
    Leaf(LeafId),
    Tip { version: Version, root: ObjectId },
    Snapshot(Snapshot),
    Bytes(Option<Vec<u8>>),
    Grants(Vec<Grant>),
    Grant(Grant),
    Object(ObjectId),
    View(LeafView),
    Views(Vec<LeafView>),
    Proposal(Proposal),
    Candidate(Candidate),
    Receipt(Receipt),
    Result(Option<Receipt>),
    Maintenance(Maintenance),
}
#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Response {
    Ok { output: Output },
    Error { message: String },
}
fn execute(root: &Path, command: Command) -> cowtree_metadata::Result<Output> {
    if let Command::Init { limits } = command {
        Store::create(root, limits)?;
        return Ok(Output::Done);
    }
    let mut store = Store::open(root)?;
    let output = match command {
        Command::Init { .. } => return Err(cowtree_metadata::Error::Schema),
        Command::CreateLeaf => Output::Leaf(store.create_leaf()?),
        Command::Tip => {
            let (version, root) = store.tip()?;
            Output::Tip { version, root }
        }
        Command::Snapshot { version } => Output::Snapshot(store.snapshot(version)?),
        Command::Read { version, path } => Output::Bytes(store.read(version, &path)?),
        Command::Acquire { leaf, paths } => Output::Grants(store.acquire(leaf, &paths)?),
        Command::Activate { grant } => Output::Grant(store.activate(&grant)?),
        Command::Stage { leaf, data } => Output::Object(store.stage(leaf, &data)?),
        Command::Edit { grant, value } => Output::View(store.edit(&grant, value)?),
        Command::View { leaf } => Output::Views(store.view(leaf)?),
        Command::Release { grant } => store.release(&grant).map(|()| Output::Done)?,
        Command::Revoke { path } => store.revoke(&path).map(|()| Output::Done)?,
        Command::Discard { leaf, path } => store.discard(leaf, &path).map(|()| Output::Done)?,
        Command::DiscardUpload { leaf, object } => {
            store.discard_upload(leaf, &object).map(|()| Output::Done)?
        }
        Command::DropLeaf { leaf } => store.drop_leaf(leaf).map(|()| Output::Done)?,
        Command::Propose { input } => Output::Proposal(store.propose(input)?),
        Command::Prepare { request } => Output::Candidate(store.prepare(request)?),
        Command::Commit { candidate } => Output::Receipt(store.commit(candidate)?),
        Command::Result { request } => Output::Result(store.result(request)?),
        Command::Abort { request } => store.abort(request).map(|()| Output::Done)?,
        Command::Retain { version } => store.retain(version).map(|()| Output::Done)?,
        Command::ReleaseRetention { version } => {
            store.release_retention(version).map(|()| Output::Done)?
        }
        Command::Maintain => Output::Maintenance(store.maintain()?),
    };
    Ok(output)
}
pub fn run(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    loop {
        let mut line = Vec::new();
        // A bounded line prevents an incomplete JSON request from consuming all memory.
        let count =
            io::Read::take(&mut input, 128 * 1024 * 1024 + 1).read_until(b'\n', &mut line)?;
        if count == 0 {
            return Ok(());
        }
        if line.len() > 128 * 1024 * 1024 {
            return Err("JSON request exceeds 128 MiB".into());
        }
        let response = match serde_json::from_slice::<Command>(&line) {
            Ok(command) => match execute(root, command) {
                Ok(result) => Response::Ok { output: result },
                Err(error) => Response::Error { message: error.to_string() },
            },
            Err(error) => Response::Error { message: error.to_string() },
        };
        serde_json::to_writer(&mut output, &response)?;
        writeln!(output)?;
        output.flush()?;
    }
}
