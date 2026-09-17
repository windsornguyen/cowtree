// Copyright (c) 2026 Windsor Nguyen

//! Line-delimited JSON interface to the local metadata authority.

use cowtree_metadata::{
    BatchCandidate, BatchReceipt, Candidate, Entry, ErrorCode, ErrorDetails, Grant, LeafId,
    LeafView, Limits, Maintenance, Proposal, ProposalInput, Receipt, RequestId, ResolutionInput,
    ResourcePath, RetryAction, Snapshot, Store, Version, WireError, objects::ObjectId,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
    Init {
        #[serde(default)]
        limits: Limits,
    },
    CreateLeaf,
    Leaves,
    Grants,
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
    StageFile {
        leaf: LeafId,
        path: PathBuf,
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
    ReplaceClientPins {
        objects: BTreeSet<ObjectId>,
    },
    Maintain,
    PrepareBatch {
        requests: Vec<RequestId>,
        expected_tip: Version,
    },
    CommitBatch {
        candidate: BatchCandidate,
    },
    Resolve {
        input: ResolutionInput,
    },
}
#[derive(Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
enum Output {
    Done,
    Leaf(LeafId),
    Leaves(Vec<LeafId>),
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
    BatchCandidate(BatchCandidate),
    BatchReceipt(BatchReceipt),
    Receipt(Receipt),
    Result(Option<Receipt>),
    Maintenance(Maintenance),
}
#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Response {
    Ok {
        output: Output,
    },
    Error {
        #[serde(flatten)]
        error: WireError,
    },
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
        Command::Leaves => Output::Leaves(store.leaves()?),
        Command::Grants => Output::Grants(store.grants()?),
        Command::Tip => {
            let (version, root) = store.tip()?;
            Output::Tip { version, root }
        }
        Command::Snapshot { version } => Output::Snapshot(store.snapshot(version)?),
        Command::Read { version, path } => Output::Bytes(store.read(version, &path)?),
        Command::Acquire { leaf, paths } => Output::Grants(store.acquire(leaf, &paths)?),
        Command::Activate { grant } => Output::Grant(store.activate(&grant)?),
        Command::Stage { leaf, data } => Output::Object(store.stage(leaf, &data)?),
        Command::StageFile { leaf, path } => Output::Object(store.stage_file(leaf, &path)?),
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
        Command::ReplaceClientPins { objects } => {
            store.replace_client_pins(&objects).map(|()| Output::Done)?
        }
        Command::Maintain => Output::Maintenance(store.maintain()?),
        Command::PrepareBatch { requests, expected_tip } => {
            Output::BatchCandidate(store.prepare_batch(requests, expected_tip)?)
        }
        Command::CommitBatch { candidate } => Output::BatchReceipt(store.commit_batch(candidate)?),
        Command::Resolve { input } => Output::Proposal(store.resolve(input)?),
    };
    Ok(output)
}
const MAX_REQUEST_BYTES: usize = 128 * 1024 * 1024;

fn invalid_request(error: serde_json::Error) -> Response {
    Response::Error {
        error: WireError {
            message: error.to_string(),
            code: ErrorCode::InvalidRequest,
            retry_action: RetryAction::None,
            details: ErrorDetails::InvalidRequest { line: error.line(), column: error.column() },
        },
    }
}

fn oversized_request() -> Response {
    Response::Error {
        error: WireError {
            message: "JSON request exceeds 128 MiB".into(),
            code: ErrorCode::InvalidRequest,
            retry_action: RetryAction::None,
            details: ErrorDetails::RequestTooLarge { max_bytes: MAX_REQUEST_BYTES },
        },
    }
}

// Drain without retaining bytes, preserving the next line as a separate request.
fn drain_line(input: &mut impl BufRead) -> io::Result<()> {
    loop {
        let bytes = input.fill_buf()?;
        if bytes.is_empty() {
            return Ok(());
        }
        let newline = bytes.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(bytes.len(), |index| index + 1);
        input.consume(consumed);
        if newline.is_some() {
            return Ok(());
        }
    }
}

fn respond(root: &Path, line: &[u8]) -> Response {
    match serde_json::from_slice::<Command>(line) {
        Ok(command) => match execute(root, command) {
            Ok(result) => Response::Ok { output: result },
            Err(error) => Response::Error { error: error.wire() },
        },
        Err(error) => invalid_request(error),
    }
}

pub fn run(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    loop {
        let mut line = Vec::new();
        let count = io::Read::take(&mut input, MAX_REQUEST_BYTES as u64 + 1)
            .read_until(b'\n', &mut line)?;
        if count == 0 {
            return Ok(());
        }
        let response = if line.len() > MAX_REQUEST_BYTES {
            if line.last() != Some(&b'\n') {
                drain_line(&mut input)?;
            }
            oversized_request()
        } else {
            respond(root, &line)
        };
        serde_json::to_writer(&mut output, &response)?;
        writeln!(output)?;
        output.flush()?;
    }
}
