#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use cowtree_metadata::objects::ObjectId;
use cowtree_metadata::{
    Candidate, Entry, EntryKind, Error, Grant, LeafId, Limits, ProposalInput, Receipt, RequestId,
    ResourcePath, Result, Snapshot, Store, Version,
};

#[derive(Clone, Copy, Debug)]
enum Action {
    Edit,
    Propose,
    Commit,
    Revoke,
    Drop,
    Maintain,
    Reopen,
    Replay,
}

const ACTIONS: [Action; 10] = [
    Action::Edit,
    Action::Edit,
    Action::Propose,
    Action::Commit,
    Action::Commit,
    Action::Revoke,
    Action::Drop,
    Action::Maintain,
    Action::Reopen,
    Action::Replay,
];

struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn choose(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
}

struct Pending {
    request: RequestId,
    token: cowtree_metadata::Token,
    origin: Option<Entry>,
    value: Option<Entry>,
    candidate: Option<Candidate>,
}

struct Leaf {
    id: LeafId,
    grant: Grant,
    origin: Option<Entry>,
    value: Option<Entry>,
    revision: u64,
    sequence: u64,
    pending: Option<Pending>,
}

struct Model {
    root: PathBuf,
    store: Store,
    leaves: Vec<Leaf>,
    tip: Snapshot,
    version: Version,
    bytes: BTreeMap<ObjectId, Vec<u8>>,
    committed: Vec<(Candidate, Receipt)>,
    random: Random,
}

impl Model {
    fn create(root: PathBuf, seed: u64) -> Result<Self> {
        let mut store = Store::create(&root, Limits::default())?;
        let mut leaves = Vec::new();
        for index in 0..4 {
            let path = ResourcePath::parse(format!("slot-{index}/file"))?;
            leaves.push(new_leaf(&mut store, path, None)?);
        }
        let mut model = Self {
            root,
            store,
            leaves,
            tip: Snapshot::new(),
            version: Version::new(0)?,
            bytes: BTreeMap::new(),
            committed: Vec::new(),
            random: Random(seed),
        };
        for index in 0..4 {
            model.edit(index)?;
            model.propose(index)?;
            model.prepare(index)?;
        }
        Ok(model)
    }

    fn edit(&mut self, index: usize) -> Result<()> {
        let leaf = &mut self.leaves[index];
        let value = if self.random.choose(4) == 0 {
            None
        } else {
            let bytes = format!("payload-{:016x}", self.random.next()).into_bytes();
            let id = ObjectId::from_bytes(&bytes);
            assert_eq!(self.store.stage(leaf.id, &bytes)?, id);
            self.bytes.insert(id.clone(), bytes);
            let kind = match self.random.choose(3) {
                0 => EntryKind::File,
                1 => EntryKind::Executable,
                _ => EntryKind::Symlink,
            };
            Some(Entry { object: id, kind })
        };
        let view = self.store.edit(&leaf.grant, value.clone())?;
        leaf.value = value;
        leaf.revision += 1;
        assert_eq!(view.value, leaf.value);
        assert_eq!(view.origin, leaf.origin);
        assert_eq!(view.edit_revision, leaf.revision);
        Ok(())
    }

    fn propose(&mut self, index: usize) -> Result<()> {
        let leaf = &mut self.leaves[index];
        if leaf.pending.as_ref().is_some_and(|pending| {
            pending.token != leaf.grant.token || pending.origin != leaf.origin
        }) {
            self.store.abort(leaf.pending.as_ref().unwrap().request)?;
            leaf.pending = None;
        }
        if leaf.pending.is_some() || leaf.value == leaf.origin {
            return Ok(());
        }
        let request = RequestId { leaf: leaf.id, sequence: leaf.sequence };
        let input = ProposalInput { request, paths: BTreeSet::from([leaf.grant.path.clone()]) };
        let captured = self.store.propose(input.clone())?;
        assert_eq!(captured.captured_at, self.version);
        assert_eq!(captured.changes.len(), 1);
        assert_eq!(captured.changes[0].path, leaf.grant.path);
        assert_eq!(captured.changes[0].origin, leaf.origin);
        assert_eq!(captured.changes[0].value, leaf.value);
        assert_eq!(captured.changes[0].edit_revision, leaf.revision);
        assert_eq!(self.store.propose(input)?, captured);
        leaf.pending = Some(Pending {
            request,
            token: leaf.grant.token,
            origin: leaf.origin.clone(),
            value: leaf.value.clone(),
            candidate: None,
        });
        leaf.sequence += 1;
        Ok(())
    }

    fn prepare(&mut self, index: usize) -> Result<()> {
        let leaf = &mut self.leaves[index];
        let Some(pending) = &mut leaf.pending else {
            return Ok(());
        };
        let result = self.store.prepare(pending.request);
        if pending.token != leaf.grant.token {
            assert!(matches!(result, Err(Error::StaleToken(_))));
            return Ok(());
        }
        if self.tip.get(&leaf.grant.path) != pending.origin.as_ref() {
            assert!(matches!(result, Err(Error::StaleOrigin(_))));
            return Ok(());
        }
        let candidate = result?;
        assert_eq!(candidate.parent, self.version);
        assert_eq!(candidate.request, pending.request);
        let expected = changed(&self.tip, &leaf.grant.path, &pending.value);
        assert_eq!(candidate.root, snapshot_root(&expected));
        if let Some(previous) = &pending.candidate {
            assert!(candidate.attempt > previous.attempt);
        }
        pending.candidate = Some(candidate);
        Ok(())
    }

    fn commit(&mut self, index: usize) -> Result<()> {
        let leaf = &mut self.leaves[index];
        let Some(pending) = &leaf.pending else {
            return Ok(());
        };
        let Some(candidate) = pending.candidate.clone() else {
            return Ok(());
        };
        let result = self.store.commit(candidate.clone());
        if candidate.parent != self.version {
            assert!(matches!(result, Err(Error::TipChanged { .. })));
            return Ok(());
        }
        if pending.token != leaf.grant.token {
            assert!(matches!(result, Err(Error::StaleToken(_))));
            return Ok(());
        }
        if self.tip.get(&leaf.grant.path) != pending.origin.as_ref() {
            assert!(matches!(result, Err(Error::StaleOrigin(_))));
            return Ok(());
        }
        let receipt = result?;
        self.tip = changed(&self.tip, &leaf.grant.path, &pending.value);
        self.version = Version::new(self.version.get() + 1)?;
        assert_eq!(receipt.version, self.version);
        assert_eq!(receipt.root, snapshot_root(&self.tip));
        leaf.origin = pending.value.clone();
        leaf.grant.origin = leaf.origin.clone();
        let refreshed = self.store.acquire(leaf.id, &BTreeSet::from([leaf.grant.path.clone()]))?;
        assert_eq!(refreshed, vec![leaf.grant.clone()]);
        leaf.pending = None;
        self.committed.push((candidate, receipt));
        Ok(())
    }

    fn revoke(&mut self, index: usize) -> Result<()> {
        let leaf = &mut self.leaves[index];
        let old = leaf.grant.clone();
        let mut other = Store::open(&self.root)?;
        other.revoke(&old.path)?;
        other.discard(leaf.id, &old.path)?;
        let grant = other.acquire(leaf.id, &BTreeSet::from([old.path.clone()]))?.remove(0);
        leaf.grant = other.activate(&grant)?;
        assert!(leaf.grant.token > old.token);
        leaf.origin = self.tip.get(&old.path).cloned();
        leaf.value = leaf.origin.clone();
        leaf.revision = 0;
        assert_eq!(leaf.grant.origin, leaf.origin);
        assert!(matches!(self.store.edit(&old, None), Err(Error::StaleToken(_))));
        Ok(())
    }

    fn drop_leaf(&mut self, index: usize) -> Result<()> {
        let old = &self.leaves[index];
        let old_id = old.id;
        let old_token = old.grant.token;
        let path = old.grant.path.clone();
        self.store.drop_leaf(old_id)?;
        assert!(matches!(self.store.view(old_id), Err(Error::LeafInactive(_))));
        let new = new_leaf(&mut self.store, path.clone(), self.tip.get(&path).cloned())?;
        assert!(new.id > old_id);
        assert!(new.grant.token > old_token);
        self.leaves[index] = new;
        Ok(())
    }

    fn replay(&mut self) -> Result<()> {
        if self.committed.is_empty() {
            return Ok(());
        }
        let index = self.random.choose(self.committed.len());
        let (candidate, receipt) = &self.committed[index];
        assert_eq!(self.store.commit(candidate.clone())?, *receipt);
        assert_eq!(self.store.result(candidate.request)?, Some(receipt.clone()));
        Ok(())
    }

    fn step(&mut self, action: Action, leaf: usize) -> Result<()> {
        match action {
            Action::Edit => self.edit(leaf)?,
            Action::Propose => {
                self.propose(leaf)?;
                self.prepare(leaf)?;
            }
            Action::Commit => self.commit(leaf)?,
            Action::Revoke => self.revoke(leaf)?,
            Action::Drop => self.drop_leaf(leaf)?,
            Action::Maintain => {
                self.store.maintain()?;
            }
            Action::Reopen => {
                self.store = Store::open(&self.root)?;
            }
            Action::Replay => self.replay()?,
        }
        self.verify()
    }

    fn verify(&mut self) -> Result<()> {
        assert_eq!(self.store.tip()?, (self.version, snapshot_root(&self.tip)));
        assert_eq!(self.store.snapshot(self.version)?, self.tip);
        for (path, entry) in &self.tip {
            assert_eq!(
                self.store.read(self.version, path)?,
                Some(self.bytes[&entry.object].clone())
            );
        }
        for leaf in &self.leaves {
            let view = self.store.view(leaf.id)?;
            assert_eq!(view.len(), 1);
            assert_eq!(view[0].path, leaf.grant.path);
            assert_eq!(view[0].origin, leaf.origin);
            assert_eq!(view[0].value, leaf.value);
            assert_eq!(view[0].edit_revision, leaf.revision);
            assert_eq!(view[0].dirty(), leaf.value != leaf.origin);
        }
        Ok(())
    }
}

fn new_leaf(store: &mut Store, path: ResourcePath, origin: Option<Entry>) -> Result<Leaf> {
    let id = store.create_leaf()?;
    let grant = store.acquire(id, &BTreeSet::from([path]))?.remove(0);
    let grant = store.activate(&grant)?;
    assert_eq!(grant.origin, origin);
    Ok(Leaf { id, grant, value: origin.clone(), origin, revision: 0, sequence: 1, pending: None })
}

fn changed(snapshot: &Snapshot, path: &ResourcePath, value: &Option<Entry>) -> Snapshot {
    let mut next = snapshot.clone();
    match value {
        Some(entry) => {
            next.insert(path.clone(), entry.clone());
        }
        None => {
            next.remove(path);
        }
    }
    next
}

fn snapshot_root(snapshot: &Snapshot) -> ObjectId {
    ObjectId::from_bytes(&serde_json::to_vec(snapshot).unwrap())
}

#[test]
fn seeded_teardown_histories_preserve_published_bytes_and_leaf_intent() {
    for seed in [1, 7, 31, 127, 509, 1021, 4093, 65521] {
        let directory = tempfile::tempdir().unwrap();
        let mut model = Model::create(directory.path().join("store"), seed).unwrap();
        for round in 0..50 {
            let action = ACTIONS[model.random.choose(ACTIONS.len())];
            let leaf = model.random.choose(4);
            eprintln!("seed={seed} round={round} action={action:?} leaf={leaf}");
            model.step(action, leaf).unwrap();
        }
        for leaf in 0..4 {
            eprintln!("seed={seed} final-drain leaf={leaf}");
            model.propose(leaf).unwrap();
            model.prepare(leaf).unwrap();
            model.commit(leaf).unwrap();
            model.verify().unwrap();
        }
        assert!(!model.committed.is_empty(), "seed={seed} must exercise publication");
    }
}
