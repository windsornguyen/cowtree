// Copyright (c) 2026 Windsor Nguyen

#![allow(clippy::unwrap_used)]

use cowtree_metadata::{Limits, ResourcePath, Store};
use std::fs;

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

#[test]
fn published_authority_contains_complete_configuration_and_opens_for_import() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("file"), b"tracked bytes").unwrap();
    let root = temp.path().join("workspace");
    let mut store = Store::create_workspace(&root, &source, COMMIT, Limits::default()).unwrap();
    let config: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("workspace.json")).unwrap()).unwrap();
    assert_eq!(
        config,
        serde_json::json!({"format":1,"source":source.canonicalize().unwrap(),
        "commit":COMMIT,"initial_version":null})
    );
    assert_eq!(store.tip().unwrap().0.get(), 0);
    store.import_tree(&source, &[ResourcePath::parse("file").unwrap()]).unwrap();
    drop(store);
    assert_eq!(Store::open(&root).unwrap().tip().unwrap().0.get(), 1);
    assert!(
        fs::read_dir(temp.path()).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".cowtree-init-"))
    );
}

#[test]
fn existing_destinations_are_never_replaced() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    let root = temp.path().join("workspace");
    drop(Store::create_workspace(&root, &source, COMMIT, Limits::default()).unwrap());
    let original = fs::read(root.join("workspace.json")).unwrap();
    assert!(Store::create_workspace(&root, &source, &"a".repeat(64), Limits::default()).is_err());
    assert_eq!(fs::read(root.join("workspace.json")).unwrap(), original);
    assert_eq!(Store::open(&root).unwrap().tip().unwrap().0.get(), 0);
    let empty = temp.path().join("empty");
    fs::create_dir(&empty).unwrap();
    assert!(Store::create_workspace(&empty, &source, COMMIT, Limits::default()).is_err());
    assert_eq!(fs::read_dir(empty).unwrap().count(), 0);
}

#[test]
fn invalid_identity_or_source_never_exposes_a_destination() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    let root = temp.path().join("workspace");
    for commit in ["".to_owned(), "a".repeat(39), "g".repeat(40), "a".repeat(65)] {
        assert!(Store::create_workspace(&root, &source, &commit, Limits::default()).is_err());
        assert!(!root.exists());
    }
    assert!(
        Store::create_workspace(&root, &source.join("missing"), COMMIT, Limits::default()).is_err()
    );
    assert!(!root.exists());
    Store::create_workspace(&root, &source, &"b".repeat(64), Limits::default()).unwrap();
}

#[test]
#[cfg(feature = "fault-injection")]
fn crash_publication_boundary_exposes_either_nothing_or_a_complete_authority() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    for (point, published) in
        [("before-authority-publish", false), ("after-authority-publish", true)]
    {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("file"), b"tracked bytes").unwrap();
        let root = temp.path().join("workspace");
        let mut child = Command::new(env!("CARGO_BIN_EXE_cowtree-metadata"))
            .arg(&root)
            .env("COWTREE_CRASH_AT", point)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        serde_json::to_writer(
            &mut stdin,
            &serde_json::json!({"op":"init_workspace",
            "source":source,"commit":COMMIT}),
        )
        .unwrap();
        writeln!(stdin).unwrap();
        drop(stdin);
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(73), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(output.stdout.is_empty());
        assert_eq!(root.exists(), published);
        if !published {
            drop(Store::create_workspace(&root, &source, COMMIT, Limits::default()).unwrap());
        }
        let config: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("workspace.json")).unwrap()).unwrap();
        assert_eq!(config["initial_version"], serde_json::Value::Null);
        let mut reopened = Store::open(&root).unwrap();
        reopened.import_tree(&source, &[ResourcePath::parse("file").unwrap()]).unwrap();
        assert_eq!(reopened.tip().unwrap().0.get(), 1);
    }
}

#[test]
fn concurrent_initializers_publish_exactly_one_configuration() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    let root = temp.path().join("workspace");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let handles: Vec<_> = ["a".repeat(40), "b".repeat(40)]
        .into_iter()
        .map(|commit| {
            let root = root.clone();
            let source = source.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                let success =
                    Store::create_workspace(&root, &source, &commit, Limits::default()).is_ok();
                (commit, success)
            })
        })
        .collect();
    let winners: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .filter(|(_, success)| *success)
        .collect();
    assert_eq!(winners.len(), 1);
    let config: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("workspace.json")).unwrap()).unwrap();
    assert_eq!(config["commit"], winners[0].0);
    Store::open(&root).unwrap();
}

#[test]
fn managed_bootstrap_does_not_admit_leaf_writes_before_import() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("source");
    std::fs::create_dir(&source).unwrap();
    let mut store = Store::create_workspace(
        &temporary.path().join("authority"),
        &source,
        &"a".repeat(40),
        Limits::default(),
    )
    .unwrap();
    assert!(store.create_leaf().is_err());
    store.import_tree(&source, &[]).unwrap();
    store.create_leaf().unwrap();
}
