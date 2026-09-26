// Copyright (c) 2026 Windsor Nguyen

//! Imported read-only namespaces reject every overlapping grant without consuming state.

use cowtree_metadata::{Entry, EntryKind, Limits, ResourcePath, Store, objects::ObjectId};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fs,
};

type TestResult = Result<(), Box<dyn Error>>;

fn readonly(scope: &str, bytes: &[u8]) -> Result<Entry, serde_json::Error> {
    serde_json::from_value(serde_json::json!({
        "object": ObjectId::from_bytes(bytes),
        "kind": {"read_only": {"file_kind": "file", "scope": scope}},
    }))
}

#[test]
fn immutable_namespaces_survive_reopen_and_refuse_atomic_grant_batches() -> TestResult {
    let temporary = tempfile::tempdir()?;
    let source = temporary.path().join("source");
    let authority = temporary.path().join("authority");
    fs::create_dir_all(source.join("vendor/dep"))?;
    fs::write(source.join("vendor/dep/pin"), b"dependency")?;
    fs::write(source.join("app"), b"editable")?;
    let manifest = BTreeMap::from([
        (ResourcePath::parse("vendor/dep/pin")?, readonly("vendor/dep", b"dependency")?),
        (
            ResourcePath::parse("app")?,
            Entry { object: ObjectId::from_bytes(b"editable"), kind: EntryKind::File },
        ),
    ]);
    let mut store = Store::create(&authority, Limits::default())?;
    let progress = store.begin_import(manifest.clone())?;
    store.import_chunk(&progress.root, &source)?;
    store.finish_import(&progress.root)?;
    drop(store);
    let mut store = Store::open(&authority)?;
    let leaf = store.create_leaf()?;
    for path in ["vendor", "vendor/dep", "vendor/dep/pin", "vendor/dep/new"] {
        let paths = BTreeSet::from([ResourcePath::parse("app")?, ResourcePath::parse(path)?]);
        let error = store.acquire(leaf, &paths).err().ok_or("read-only grant was accepted")?;
        assert_eq!(serde_json::to_value(error.wire())?["code"], "read_only_path");
        assert!(store.grants()?.is_empty());
    }
    let paths = BTreeSet::from([ResourcePath::parse("app")?]);
    let grant = store.acquire(leaf, &paths)?.pop().ok_or("editable grant was absent")?;
    assert_eq!(grant.token.get(), 1);
    let grant = store.activate(&grant)?;
    let object = store.stage(leaf, b"replacement")?;
    let mut forbidden = readonly("app", b"replacement")?;
    forbidden.object = object;
    let error = store.edit(&grant, Some(forbidden)).err().ok_or("policy mutation was accepted")?;
    assert_eq!(serde_json::to_value(error.wire())?["code"], "read_only_path");
    assert_eq!(store.snapshot(store.tip()?.0)?, manifest);
    let adjacent = BTreeSet::from([ResourcePath::parse("vendor/dependency/new")?]);
    assert_eq!(store.acquire(leaf, &adjacent)?.len(), 1);
    Ok(())
}

#[test]
fn a_readonly_scope_must_contain_its_entry() -> TestResult {
    let temporary = tempfile::tempdir()?;
    let mut store = Store::create(&temporary.path().join("authority"), Limits::default())?;
    let manifest = BTreeMap::from([(
        ResourcePath::parse("vendor/dep/pin")?,
        readonly("unrelated", b"dependency")?,
    )]);
    let error = store.begin_import(manifest).err().ok_or("unrelated scope was accepted")?;
    assert_eq!(serde_json::to_value(error.wire())?["code"], "invalid_read_only_scope");
    assert_eq!(store.tip()?.0.get(), 0);
    assert!(store.status_import()?.is_none());
    Ok(())
}

#[test]
fn readonly_kinds_fail_closed_for_legacy_readers() -> TestResult {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum LegacyKind {
        File,
        Executable,
        Symlink,
    }
    let value = serde_json::to_value(readonly("vendor/dep", b"dependency")?)?;
    assert!(serde_json::from_value::<LegacyKind>(value["kind"].clone()).is_err());
    assert_eq!(serde_json::to_value(EntryKind::File)?, "file");
    Ok(())
}
