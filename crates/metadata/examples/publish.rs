// Copyright (c) 2026 Windsor Nguyen

//! Publish a file, retain a later local edit, and reopen the committed snapshot.

use cowtree_metadata::{Entry, EntryKind, Limits, ProposalInput, RequestId, ResourcePath, Store};
use std::collections::BTreeSet;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let root = directory.path().join("workspace");
    let mut store = Store::create(&root, Limits::default())?;
    let leaf = store.create_leaf()?;
    let path = ResourcePath::parse("src/main.rs")?;
    let paths = BTreeSet::from([path.clone()]);
    let grant = store.acquire(leaf, &paths)?.remove(0);
    let grant = store.activate(&grant)?;
    let object = store.stage(leaf, b"fn main() {}\n")?;
    store.edit(&grant, Some(Entry { object, kind: EntryKind::File }))?;
    let request = RequestId { leaf, sequence: 1 };
    store.propose(ProposalInput { request, paths })?;
    let candidate = store.prepare(request)?;
    let object = store.stage(leaf, b"fn main() { println!(\"later edit\"); }\n")?;
    store.edit(&grant, Some(Entry { object, kind: EntryKind::File }))?;
    let receipt = store.commit(candidate.clone())?;
    assert_eq!(store.commit(candidate)?, receipt);
    assert!(store.view(leaf)?[0].dirty());
    drop(store);
    let mut reopened = Store::open(&root)?;
    assert_eq!(reopened.read(receipt.version, &path)?, Some(b"fn main() {}\n".to_vec()));
    println!("SQLite {}: {}", reopened.sqlite_version(), serde_json::to_string(&receipt)?);
    Ok(())
}
