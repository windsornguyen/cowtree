// Copyright (c) 2026 Windsor Nguyen

#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::path::Path;

use cowtree_metadata::objects::{ObjectError, ObjectId};
use cowtree_metadata::{
    Candidate, Entry, EntryKind, Error, Grant, LeafId, Limits, Proposal, ProposalInput, RequestId,
    Store, Version,
};

fn paths(values: &[&str]) -> BTreeSet<cowtree_metadata::ResourcePath> {
    values.iter().map(|value| cowtree_metadata::ResourcePath::parse(*value).unwrap()).collect()
}

fn open_store(root: &Path) -> Store {
    Store::create(root, Limits::default()).unwrap()
}

fn grant(store: &mut Store, leaf: LeafId, path: &str) -> Grant {
    let reserved = store.acquire(leaf, &paths(&[path])).unwrap().remove(0);
    store.activate(&reserved).unwrap()
}

fn edit(store: &mut Store, grant: &Grant, bytes: &[u8]) -> Entry {
    let entry = Entry { object: store.stage(grant.leaf, bytes).unwrap(), kind: EntryKind::File };
    store.edit(grant, Some(entry.clone())).unwrap();
    entry
}

fn capture(store: &mut Store, leaf: LeafId, sequence: u64, names: &[&str]) -> Proposal {
    store
        .propose(ProposalInput { request: RequestId { leaf, sequence }, paths: paths(names) })
        .unwrap()
}

fn prepare(store: &mut Store, leaf: LeafId, sequence: u64, names: &[&str]) -> Candidate {
    let proposal = capture(store, leaf, sequence, names);
    store.prepare(proposal.request).unwrap()
}

#[test]
fn disjoint_proposals_reprepare_on_the_new_tip_without_losing_either_change() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let mut first = open_store(&root);
    let mut second = Store::open(&root).unwrap();
    let left = first.create_leaf().unwrap();
    let right = second.create_leaf().unwrap();
    let left_grant = grant(&mut first, left, "left");
    let right_grant = grant(&mut second, right, "right");
    let left_entry = edit(&mut first, &left_grant, b"left edit");
    let right_entry = edit(&mut second, &right_grant, b"right edit");
    let left_candidate = prepare(&mut first, left, 1, &["left"]);
    let right_candidate = prepare(&mut second, right, 1, &["right"]);
    assert_eq!(left_candidate.parent, right_candidate.parent);
    let receipt = first.commit(left_candidate).unwrap();
    assert!(matches!(
        second.commit(right_candidate.clone()),
        Err(Error::TipChanged { expected: 0, actual: 1 })
    ));
    assert_eq!(second.tip().unwrap().0, receipt.version);
    assert_eq!(second.result(right_candidate.request).unwrap(), None);
    let replacement = second.prepare(right_candidate.request).unwrap();
    assert!(replacement.attempt > right_candidate.attempt);
    let receipt = second.commit(replacement).unwrap();
    let snapshot = first.snapshot(receipt.version).unwrap();
    assert_eq!(snapshot.len(), 2);
    assert_eq!(snapshot.get(&left_grant.path), Some(&left_entry));
    assert_eq!(snapshot.get(&right_grant.path), Some(&right_entry));
    assert_eq!(first.read(receipt.version, &left_grant.path).unwrap().unwrap(), b"left edit");
    assert_eq!(first.read(receipt.version, &right_grant.path).unwrap().unwrap(), b"right edit");
}

#[test]
fn overlapping_proposals_from_one_owner_cannot_overwrite_a_changed_origin() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let mut first = open_store(&root);
    let mut second = Store::open(&root).unwrap();
    let leaf = first.create_leaf().unwrap();
    let reservation = grant(&mut first, leaf, "file");
    let a = edit(&mut first, &reservation, b"a");
    let earlier = prepare(&mut first, leaf, 1, &["file"]);
    edit(&mut second, &reservation, b"b");
    let later = prepare(&mut second, leaf, 2, &["file"]);
    let receipt = first.commit(earlier).unwrap();
    assert!(matches!(second.commit(later.clone()), Err(Error::TipChanged { .. })));
    assert!(matches!(second.prepare(later.request), Err(Error::StaleOrigin(_))));
    assert_eq!(second.snapshot(receipt.version).unwrap().get(&reservation.path), Some(&a));
    assert!(second.view(leaf).unwrap()[0].dirty());
    assert_eq!(second.result(later.request).unwrap(), None);
}

#[test]
fn committing_captured_a_preserves_later_dirty_b_and_its_revision() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let mut first = open_store(&root);
    let mut second = Store::open(&root).unwrap();
    let leaf = first.create_leaf().unwrap();
    let reservation = grant(&mut first, leaf, "file");
    let a = edit(&mut first, &reservation, b"a");
    let frozen = capture(&mut first, leaf, 1, &["file"]);
    let b = edit(&mut second, &reservation, b"b");
    let edited = second.view(leaf).unwrap().remove(0);
    let candidate = first.prepare(frozen.request).unwrap();
    let receipt = first.commit(candidate).unwrap();
    let view = second.view(leaf).unwrap().remove(0);
    assert_eq!(view.origin, Some(a.clone()));
    assert_eq!(view.value, Some(b.clone()));
    assert_eq!(view.edit_revision, edited.edit_revision);
    assert!(view.dirty());
    assert_eq!(first.snapshot(receipt.version).unwrap().get(&reservation.path), Some(&a));
    let refreshed = grant(&mut second, leaf, "file");
    assert_eq!(refreshed.token, reservation.token);
    let candidate = prepare(&mut second, leaf, 2, &["file"]);
    let receipt = second.commit(candidate).unwrap();
    assert_eq!(first.snapshot(receipt.version).unwrap().get(&reservation.path), Some(&b));
    assert!(!first.view(leaf).unwrap()[0].dirty());
}

#[test]
fn request_replay_keeps_the_original_capture_and_rejects_changed_paths() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let mut first = open_store(&root);
    let mut second = Store::open(&root).unwrap();
    let leaf = first.create_leaf().unwrap();
    let reservation = grant(&mut first, leaf, "file");
    edit(&mut first, &reservation, b"original capture");
    let frozen = capture(&mut first, leaf, 1, &["file"]);
    edit(&mut second, &reservation, b"later edit");
    assert_eq!(capture(&mut second, leaf, 1, &["file"]), frozen);
    let changed = ProposalInput { request: frozen.request, paths: paths(&["other"]) };
    assert!(matches!(second.propose(changed.clone()), Err(Error::RequestConflict)));
    let candidate = first.prepare(frozen.request).unwrap();
    first.commit(candidate).unwrap();
    drop(first);
    let mut reopened = Store::open(&root).unwrap();
    assert_eq!(capture(&mut reopened, leaf, 1, &["file"]), frozen);
    assert!(matches!(reopened.propose(changed), Err(Error::RequestConflict)));
}

#[test]
fn only_the_latest_exact_prepared_candidate_can_commit() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let mut first = open_store(&root);
    let mut second = Store::open(&root).unwrap();
    let leaf = first.create_leaf().unwrap();
    let reservation = grant(&mut first, leaf, "file");
    edit(&mut first, &reservation, b"candidate");
    let earlier = prepare(&mut first, leaf, 1, &["file"]);
    let latest = second.prepare(earlier.request).unwrap();
    assert!(latest.attempt > earlier.attempt);
    assert_eq!(latest.root, earlier.root);
    let forgeries = [
        Candidate { root: ObjectId::from_bytes(b"wrong root"), ..latest.clone() },
        Candidate { parent: Version::new(1).unwrap(), ..latest.clone() },
        Candidate { attempt: u64::MAX, ..latest.clone() },
        earlier,
    ];
    for forged in forgeries {
        assert!(matches!(first.commit(forged), Err(Error::CandidateMismatch)));
        assert_eq!(first.tip().unwrap().0, Version::new(0).unwrap());
        assert_eq!(first.result(latest.request).unwrap(), None);
    }
    let receipt = second.commit(latest.clone()).unwrap();
    assert_eq!(first.commit(latest).unwrap(), receipt);
    assert_eq!(first.tip().unwrap().0, Version::new(1).unwrap());
}

#[test]
fn revoked_tokens_block_both_preparation_and_publication() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let mut first = open_store(&root);
    let mut second = Store::open(&root).unwrap();
    let leaf = first.create_leaf().unwrap();
    let next_owner = second.create_leaf().unwrap();
    let reservation = grant(&mut first, leaf, "file");
    edit(&mut first, &reservation, b"stale owner");
    let candidate = prepare(&mut first, leaf, 1, &["file"]);
    let retained = first.view(leaf).unwrap();
    second.revoke(&reservation.path).unwrap();
    let replacement = grant(&mut second, next_owner, "file");
    assert!(replacement.token > reservation.token);
    assert!(matches!(first.prepare(candidate.request), Err(Error::StaleToken(_))));
    assert!(matches!(first.commit(candidate.clone()), Err(Error::StaleToken(_))));
    assert_eq!(second.tip().unwrap().0, Version::new(0).unwrap());
    assert_eq!(first.view(leaf).unwrap(), retained);
    assert_eq!(first.result(candidate.request).unwrap(), None);
}

#[test]
fn absent_or_corrupt_referenced_bytes_never_advance_the_tip() {
    for corrupt in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("store");
        let mut store = open_store(&root);
        let leaf = store.create_leaf().unwrap();
        let reservation = grant(&mut store, leaf, "file");
        let entry = edit(&mut store, &reservation, b"original bytes");
        let candidate = prepare(&mut store, leaf, 1, &["file"]);
        let object = root.join("objects").join(entry.object.as_str());
        if corrupt {
            std::fs::write(object, b"damaged bytes").unwrap();
        } else {
            std::fs::remove_file(object).unwrap();
        }
        let mut reopened = Store::open(&root).unwrap();
        let result = reopened.commit(candidate.clone());
        if corrupt {
            assert!(matches!(result, Err(Error::Object(ObjectError::Corrupt { .. }))));
        } else {
            assert!(matches!(result, Err(Error::Object(ObjectError::Missing { .. }))));
        }
        assert_eq!(reopened.tip().unwrap().0, Version::new(0).unwrap());
        assert_eq!(reopened.result(candidate.request).unwrap(), None);
        assert!(reopened.view(leaf).unwrap()[0].dirty());
    }
}

#[test]
fn corrupt_prepared_snapshot_bytes_never_advance_the_tip() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let mut store = open_store(&root);
    let leaf = store.create_leaf().unwrap();
    let reservation = grant(&mut store, leaf, "file");
    edit(&mut store, &reservation, b"entry bytes");
    let candidate = prepare(&mut store, leaf, 1, &["file"]);
    std::fs::write(root.join("objects").join(candidate.root.as_str()), b"{}").unwrap();
    let mut reopened = Store::open(&root).unwrap();
    assert!(matches!(
        reopened.commit(candidate.clone()),
        Err(Error::Object(ObjectError::Corrupt { .. }))
    ));
    assert_eq!(reopened.tip().unwrap().0, Version::new(0).unwrap());
    assert_eq!(reopened.result(candidate.request).unwrap(), None);
}

#[test]
fn a_snapshot_cannot_publish_both_a_file_and_its_descendant() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let mut store = open_store(&root);
    let leaf = store.create_leaf().unwrap();
    let ancestor = grant(&mut store, leaf, "directory");
    let descendant = grant(&mut store, leaf, "directory/file");
    edit(&mut store, &ancestor, b"not a directory");
    edit(&mut store, &descendant, b"child");
    let frozen = capture(&mut store, leaf, 1, &["directory", "directory/file"]);
    assert!(matches!(store.prepare(frozen.request), Err(Error::NamespaceConflict(_))));
    assert_eq!(store.tip().unwrap().0, Version::new(0).unwrap());
    assert_eq!(store.result(frozen.request).unwrap(), None);
    store.abort(frozen.request).unwrap();
    store.discard(leaf, &ancestor.path).unwrap();
    let candidate = prepare(&mut store, leaf, 2, &["directory/file"]);
    let receipt = store.commit(candidate).unwrap();
    let snapshot = store.snapshot(receipt.version).unwrap();
    assert_eq!(snapshot.len(), 1);
    assert!(snapshot.contains_key(&descendant.path));
}

#[test]
fn content_aba_keeps_distinct_versions_and_old_candidate_replay_is_exactly_once() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let mut store = open_store(&root);
    let leaf = store.create_leaf().unwrap();
    let mut receipts = Vec::new();
    let mut candidates = Vec::new();
    for (index, bytes) in [b"a", b"b", b"a"].into_iter().enumerate() {
        let reservation = grant(&mut store, leaf, "file");
        edit(&mut store, &reservation, bytes);
        let candidate = prepare(&mut store, leaf, index as u64 + 1, &["file"]);
        receipts.push(store.commit(candidate.clone()).unwrap());
        candidates.push(candidate);
    }
    assert_eq!(receipts[0].root, receipts[2].root);
    assert_ne!(receipts[0].root, receipts[1].root);
    assert_eq!(
        receipts.iter().map(|receipt| receipt.version.get()).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    drop(store);
    let mut reopened = Store::open(&root).unwrap();
    for (candidate, receipt) in candidates.into_iter().zip(receipts) {
        assert_eq!(reopened.commit(candidate.clone()).unwrap(), receipt);
        assert_eq!(reopened.result(candidate.request).unwrap(), Some(receipt));
        assert_eq!(reopened.tip().unwrap().0, Version::new(3).unwrap());
    }
}

#[test]
fn discard_after_capture_preserves_the_captured_proposal_and_new_local_intent() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let mut first = open_store(&root);
    let mut second = Store::open(&root).unwrap();
    let leaf = first.create_leaf().unwrap();
    let reservation = grant(&mut first, leaf, "file");
    let entry = edit(&mut first, &reservation, b"captured");
    let candidate = prepare(&mut first, leaf, 1, &["file"]);
    second.discard(leaf, &reservation.path).unwrap();
    let discarded = second.view(leaf).unwrap().remove(0);
    let receipt = first.commit(candidate).unwrap();
    let view = second.view(leaf).unwrap().remove(0);
    assert_eq!(view.value, None);
    assert_eq!(view.origin, Some(entry.clone()));
    assert!(view.dirty());
    assert_eq!(view.edit_revision, discarded.edit_revision);
    assert_eq!(second.snapshot(receipt.version).unwrap().get(&reservation.path), Some(&entry));
}

#[test]
fn regranting_the_same_owner_does_not_restore_an_old_proposals_authority() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let mut first = open_store(&root);
    let mut second = Store::open(&root).unwrap();
    let leaf = first.create_leaf().unwrap();
    let original = grant(&mut first, leaf, "file");
    edit(&mut first, &original, b"old token");
    let candidate = prepare(&mut first, leaf, 1, &["file"]);
    second.revoke(&original.path).unwrap();
    second.discard(leaf, &original.path).unwrap();
    let current = grant(&mut second, leaf, "file");
    assert_eq!(current.leaf, original.leaf);
    assert_eq!(current.origin, original.origin);
    assert!(current.token > original.token);
    assert!(matches!(first.prepare(candidate.request), Err(Error::StaleToken(_))));
    assert!(matches!(first.commit(candidate.clone()), Err(Error::StaleToken(_))));
    assert_eq!(second.tip().unwrap().0, Version::new(0).unwrap());
    assert_eq!(second.result(candidate.request).unwrap(), None);
    assert!(!second.view(leaf).unwrap()[0].dirty());
}

#[test]
fn relative_store_paths_remain_bound_to_the_original_database_directory() {
    let directory = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "relative_store_child", "--nocapture"])
        .env("COWTREE_RELATIVE_STORE_TEST", "1")
        .current_dir(directory.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "child failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn relative_store_child() {
    if std::env::var_os("COWTREE_RELATIVE_STORE_TEST").is_none() {
        return;
    }
    let original = std::env::current_dir().unwrap().join("store");
    let mut store = open_store(Path::new("store"));
    let leaf = store.create_leaf().unwrap();
    let reservation = grant(&mut store, leaf, "file");
    std::fs::create_dir("other").unwrap();
    drop(open_store(Path::new("other/store")));
    std::env::set_current_dir("other").unwrap();
    edit(&mut store, &reservation, b"belongs in original store");
    let candidate = prepare(&mut store, leaf, 1, &["file"]);
    let receipt = store.commit(candidate).unwrap();
    drop(store);
    let mut reopened = Store::open(&original).unwrap();
    assert_eq!(
        reopened.read(receipt.version, &reservation.path).unwrap().unwrap(),
        b"belongs in original store"
    );
}
