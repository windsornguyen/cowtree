// Copyright (c) 2026 Windsor Nguyen

//! Replay pinned TLC observations against native files, grants, captures, and publications.

#![cfg(unix)]
#[path = "trace/model.rs"]
mod model;

use cowtree_metadata::{
    Entry, EntryKind, LeafId, ProposalInput, ResourcePath, Store, Version, objects::ObjectId,
};
use cowtree_workspace::{CheckRequest, CreateRequest, Leaf, Policy, Workspace};
use model::{Action, Result, State, Step};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn symbolic(value: &str) -> Result<Entry> {
    Ok(Entry { object: ObjectId::parse(&model::digest(value.as_bytes()))?, kind: EntryKind::File })
}

fn observe(workspace: &Workspace, identities: &BTreeMap<String, LeafId>, state: &State) -> Result {
    let active: BTreeSet<_> =
        state.active.iter().filter(|(_, active)| **active).map(|(name, _)| name).collect();
    assert_eq!(identities.keys().collect::<BTreeSet<_>>(), active);
    let mut authority = Store::open(&workspace.root.join("authority"))?;
    assert_eq!(authority.tip()?.0.get(), u64::try_from(state.log.len())?);
    for (index, expected) in state.log.iter().enumerate() {
        let snapshot = authority.snapshot(Version::new(u64::try_from(index + 1)?)?)?;
        for (path, value) in expected {
            assert_eq!(snapshot.get(&ResourcePath::parse(path)?), Some(&symbolic(value)?));
        }
    }
    let grants = authority.grants()?;
    assert!(grants.iter().all(|grant| grant.activated));
    assert_eq!(grants.iter().map(|grant| grant.token).collect::<BTreeSet<_>>().len(), grants.len());
    let held: BTreeMap<_, _> =
        grants.iter().map(|grant| (grant.path.as_str().to_owned(), grant.leaf)).collect();
    let expected: BTreeMap<_, _> = state
        .holder
        .iter()
        .filter(|(_, holder)| *holder != "None")
        .map(|(path, holder)| (path.clone(), identities[holder]))
        .collect();
    assert_eq!(held, expected);
    let leaves: BTreeMap<_, _> =
        workspace.list()?.into_iter().map(|leaf| (leaf.id, leaf)).collect();
    for (name, identity) in identities {
        let leaf = &leaves[identity];
        let expected = &state.view[name];
        for (path, value) in expected {
            assert_eq!(fs::read_to_string(leaf.path.join(path))?, *value);
        }
        for (path, value) in &state.origin[name] {
            assert_eq!(leaf.origins.get(&ResourcePath::parse(path)?), Some(&symbolic(value)?));
        }
        observe_proposal(&mut authority, leaf, name, state)?;
        let mut baseline: BTreeMap<_, _> =
            leaf.origins.iter().map(|(path, value)| (path.clone(), Some(value.clone()))).collect();
        if let Some(pending) = &leaf.pending {
            baseline.extend(pending.changes.clone());
        }
        let mut dirty = BTreeSet::new();
        for (path, value) in expected {
            if baseline.get(&ResourcePath::parse(path)?) != Some(&Some(symbolic(value)?)) {
                dirty.insert(path.clone());
            }
        }
        assert_eq!(dirty, state.dirty[name].iter().cloned().collect());
    }
    Ok(())
}

fn observe_proposal(authority: &mut Store, leaf: &Leaf, name: &str, state: &State) -> Result {
    let proposals: Vec<_> =
        state.proposals.iter().filter(|proposal| proposal.leaf == name).collect();
    if proposals.is_empty() {
        assert!(leaf.pending.is_none());
        return Ok(());
    }
    assert_eq!(proposals.len(), 1);
    let pending = leaf.pending.as_ref().ok_or("runtime capture missing")?;
    assert!(pending.submitted);
    let model = proposals[0];
    let expected = model
        .delta
        .iter()
        .filter(|(_, value)| *value != "None")
        .map(|(path, value)| Ok((ResourcePath::parse(path)?, Some(symbolic(value)?))))
        .collect::<Result<BTreeMap<_, _>>>()?;
    assert_eq!(pending.changes, expected);
    let captured = authority.propose(ProposalInput {
        request: pending.request,
        paths: pending.changes.keys().cloned().collect(),
    })?;
    assert_eq!(captured.request, pending.request);
    assert_eq!(
        captured
            .changes
            .iter()
            .map(|change| (change.path.clone(), change.value.clone()))
            .collect::<BTreeMap<_, _>>(),
        pending.changes
    );
    for change in &captured.changes {
        assert_eq!(change.token, leaf.grants[&change.path].token);
        assert_eq!(change.origin, Some(symbolic(&model.origin[change.path.as_str()])?));
    }
    Ok(())
}

fn action(workspace: &Workspace, identities: &mut BTreeMap<String, LeafId>, step: &Step) -> Result {
    if matches!(step.action, Action::Fork) {
        let path = workspace.root.parent().ok_or("workspace has no parent")?.join(&step.leaf);
        let leaf = workspace.fork(&path, None)?;
        identities.insert(step.leaf.clone(), leaf.id);
        return Ok(());
    }
    let identity = identities[&step.leaf];
    let path = ResourcePath::parse(&step.path)?;
    match step.action {
        Action::Acquire => {
            workspace.acquire(identity, &[path])?;
        }
        Action::Write => {
            let leaf = workspace
                .list()?
                .into_iter()
                .find(|leaf| leaf.id == identity)
                .ok_or("leaf missing")?;
            fs::write(leaf.path.join(path.as_str()), &step.value)?;
        }
        Action::Propose => assert!(workspace.capture(identity)?.is_some()),
        Action::Sync => {
            workspace.sync(identity)?;
        }
        Action::Expire => Store::open(&workspace.root.join("authority"))?.revoke(&path)?,
        Action::Discard => {
            workspace.discard(identity, &[path])?;
        }
        Action::Commit => {
            let candidate = workspace.prepare(identity)?;
            let checked = workspace.check(&CheckRequest {
                supervisor: PathBuf::from(env!("CARGO_BIN_EXE_cowtree")),
                identity,
                command: vec!["/usr/bin/true".into()],
                timeout_seconds: 30,
            });
            if let Err(error) = checked {
                if let cowtree_workspace::Error::Check { log, .. } = &error {
                    eprintln!("{}", fs::read_to_string(log)?);
                }
                return Err(error.into());
            }
            let receipt = workspace.commit(&candidate)?;
            assert_eq!(workspace.result(candidate.request)?, Some(receipt.clone()));
            assert_eq!(workspace.commit(&candidate)?, receipt);
            workspace.sync(identity)?;
        }
        Action::Fork => return Err("fork dispatched as an existing leaf".into()),
    }
    Ok(())
}

fn replay(name: &str) -> Result {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize()?;
    let (manifest, export) = model::checked(&repository, name)?;
    let directory = tempfile::tempdir()?;
    let supported = cowtree::inspect_path(directory.path())?.supported();
    if std::env::var_os("COWTREE_EXPECT_SUPPORTED").is_some_and(|value| value == "1") {
        assert!(supported);
    }
    if !supported {
        eprintln!("skip native filesystem");
        return Ok(());
    }
    let source = directory.path().join("source");
    fs::create_dir(&source)?;
    let initial = &export.counterexample.state[0].1;
    for (path, value) in &initial.log[0] {
        fs::write(source.join(path), value)?;
    }
    for arguments in [
        vec!["init", "-q"],
        vec!["config", "user.name", "Model replay"],
        vec!["config", "user.email", "test@example.invalid"],
        vec!["add", "."],
        vec!["-c", "commit.gpgsign=false", "commit", "-qm", "model keys"],
    ] {
        assert!(
            Command::new("git").arg("-C").arg(&source).args(arguments).output()?.status.success()
        );
    }
    let workspace = Workspace::create(&CreateRequest {
        root: directory.path().join("workspace"),
        source,
        program: PathBuf::from(env!("CARGO_BIN_EXE_cowtree")),
        policy: Policy::default(),
    })?;
    let mut identities = BTreeMap::new();
    observe(&workspace, &identities, initial)?;
    for (index, step) in manifest.actions.iter().enumerate() {
        action(&workspace, &mut identities, step)?;
        let (number, state) = &export.counterexample.state[index + 1];
        assert_eq!(*number, u64::try_from(index + 2)?);
        observe(&workspace, &identities, state)?;
    }
    Ok(())
}

#[test]
fn stale_base_trace_preserves_disjoint_publications() -> Result {
    replay("EpochLogWitness")
}
#[test]
fn captured_commit_trace_preserves_post_capture_writes() -> Result {
    replay("EpochLogReplayCommit")
}
#[test]
fn expired_discard_trace_restores_the_captured_value() -> Result {
    replay("EpochLogReplayDiscard")
}
