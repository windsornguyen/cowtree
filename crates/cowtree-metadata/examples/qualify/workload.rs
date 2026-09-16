// Copyright (c) 2026 Windsor Nguyen

//! Four-file atomic batches, filesystem installation, and independent final oracles.

use super::{
    config::{Config, Result, random},
    fixture::{self, Source},
    measure::{self, Allocation, Latency, Reader, Readers},
};
use cowtree_metadata::{Grant, LeafId, Limits, ProposalInput, RequestId, Snapshot, Store, Version};
use serde::Serialize;
use std::{collections::BTreeSet, path::PathBuf, time::Instant};

#[derive(Serialize)]
pub struct Trial {
    pub index: u32,
    pub seed: u64,
    pub root: PathBuf,
    pub source_files: usize,
    pub batches: u32,
    pub import_ms: f64,
    pub clone_bind_ms: f64,
    pub receiver_install_ms: f64,
    pub verification_ms: f64,
    pub maintenance_ms: f64,
    pub capture: Latency,
    pub prepare: Latency,
    pub commit: Latency,
    pub install: Latency,
    pub readers: Vec<Reader>,
    pub workload_seconds: f64,
    pub batches_per_second: f64,
    pub database_bytes_before_maintenance: u64,
    pub wal_bytes_before_maintenance: u64,
    pub objects_before_maintenance: Allocation,
    pub objects_after_maintenance: Allocation,
    pub maintenance: Option<cowtree_metadata::Maintenance>,
    pub final_version: Version,
    pub verified_files: usize,
}
struct Writer {
    store: Store,
    leaf: LeafId,
    directory: PathBuf,
    expected: Snapshot,
    sequence: u64,
}

pub fn run(config: &Config, source: &Source, index: u32) -> Result<Trial> {
    let root = config.root.join(format!("trial-{index:02}"));
    std::fs::create_dir(&root)?;
    let authority = root.join("authority");
    let limits = Limits {
        max_paths: u32::try_from(source.snapshot.len())? + 4096,
        retained_epochs: config.operations + 2,
        retained_receipts: config.operations * 4 + 4,
        max_wal_bytes: 512 * 1024 * 1024,
        ..Limits::default()
    };
    let mut store = Store::create(&authority, limits)?;
    let paths: Vec<_> = source.snapshot.keys().cloned().collect();
    let start = Instant::now();
    let (version, _) = store.import_tree(&source.root, &paths)?;
    let import_ms = start.elapsed().as_secs_f64() * 1000.0;
    if store.snapshot(version)? != source.snapshot {
        return Err("import oracle mismatch".into());
    }
    let start = Instant::now();
    let directory = root.join("writer");
    fixture::clone_tree(source, &directory)?;
    let leaf = store.create_leaf()?;
    store.bind_tree(leaf, &directory, version)?;
    let clone_bind_ms = start.elapsed().as_secs_f64() * 1000.0;
    let seed = config.seed;
    let mut writer =
        Writer { store, leaf, directory, expected: source.snapshot.clone(), sequence: 1 };
    let readers = Readers::start(&authority, config.readers)?;
    let mut report = Trial {
        index,
        seed,
        root,
        source_files: paths.len(),
        batches: config.operations,
        import_ms,
        clone_bind_ms,
        receiver_install_ms: 0.0,
        verification_ms: 0.0,
        maintenance_ms: 0.0,
        capture: Latency::default(),
        prepare: Latency::default(),
        commit: Latency::default(),
        install: Latency::default(),
        readers: Vec::new(),
        workload_seconds: 0.0,
        batches_per_second: 0.0,
        database_bytes_before_maintenance: 0,
        wal_bytes_before_maintenance: 0,
        objects_before_maintenance: Allocation::default(),
        objects_after_maintenance: Allocation::default(),
        maintenance: None,
        final_version: version,
        verified_files: 0,
    };
    let start = Instant::now();
    let mut state = seed;
    for round in 0..config.operations {
        writer.round(source, &mut state, round, &mut report)?;
        if (round + 1) % 25 == 0 {
            eprintln!("trial {}: {}/{} batches", index + 1, round + 1, config.operations);
        }
    }
    report.workload_seconds = start.elapsed().as_secs_f64();
    report.batches_per_second = f64::from(config.operations) / report.workload_seconds;
    finish(writer, source, readers, report)
}

fn finish(
    mut writer: Writer,
    source: &Source,
    readers: Readers,
    mut report: Trial,
) -> Result<Trial> {
    report.readers = readers.finish()?;
    let start = Instant::now();
    writer.verify(source, &mut report)?;
    report.verification_ms = start.elapsed().as_secs_f64() * 1000.0;
    let authority = report.root.join("authority");
    report.database_bytes_before_maintenance =
        measure::length(&authority.join("metadata.sqlite3"))?;
    report.wal_bytes_before_maintenance = measure::length(&authority.join("metadata.sqlite3-wal"))?;
    report.objects_before_maintenance = measure::allocation(&authority.join("objects"))?;
    let start = Instant::now();
    report.maintenance = Some(writer.store.maintain()?);
    report.maintenance_ms = start.elapsed().as_secs_f64() * 1000.0;
    report.objects_after_maintenance = measure::allocation(&authority.join("objects"))?;
    Ok(report)
}

impl Writer {
    fn round(
        &mut self,
        source: &Source,
        state: &mut u64,
        round: u32,
        report: &mut Trial,
    ) -> Result<()> {
        let mut paths = BTreeSet::new();
        while paths.len() < 4 {
            paths.insert(source.editable[(random(state) as usize) % source.editable.len()].clone());
        }
        let reserved = self.store.acquire(self.leaf, &paths)?;
        let grants: Vec<Grant> = reserved
            .iter()
            .map(|grant| self.store.activate(grant))
            .collect::<cowtree_metadata::Result<_>>()?;
        for path in &paths {
            let marker = format!(
                "\n/* qualification seed={} round={round} path={} */\n",
                report.seed,
                path.as_str()
            );
            self.expected.insert(path.clone(), fixture::modify(&self.directory, path, &marker)?);
        }
        let start = Instant::now();
        self.store.capture_files(&grants)?;
        report.capture.record(start.elapsed());
        let mut requests = Vec::new();
        for path in &paths {
            let request = RequestId { leaf: self.leaf, sequence: self.sequence };
            self.sequence += 1;
            self.store.propose(ProposalInput { request, paths: BTreeSet::from([path.clone()]) })?;
            requests.push(request);
        }
        let start = Instant::now();
        let candidate = self.store.prepare_batch(requests, self.store.tip()?.0)?;
        report.prepare.record(start.elapsed());
        let start = Instant::now();
        let receipt = self.store.commit_batch(candidate)?;
        report.commit.record(start.elapsed());
        let version = receipt.receipts.first().ok_or("missing batch receipt")?.version;
        if receipt.receipts.iter().any(|item| item.version != version) {
            return Err("batch versions diverged".into());
        }
        let start = Instant::now();
        self.store.install(self.leaf, version)?;
        report.install.record(start.elapsed());
        if self.store.snapshot(version)? != self.expected {
            return Err("committed snapshot differs from independent oracle".into());
        }
        for grant in self.store.acquire(self.leaf, &paths)? {
            self.store.release(&grant)?;
        }
        report.final_version = version;
        Ok(())
    }
    fn verify(&mut self, source: &Source, report: &mut Trial) -> Result<()> {
        let expected_paths = self.expected.keys().cloned().collect();
        if fixture::inventory(&self.directory)? != expected_paths {
            return Err("writer path set differs from oracle".into());
        }
        let actual = fixture::snapshot(&self.directory, self.expected.keys())?;
        if actual != self.expected {
            return Err("installed tree differs from independent oracle".into());
        }
        if fixture::snapshot(&source.root, source.snapshot.keys())? != source.snapshot {
            return Err("source was modified during qualification".into());
        }
        let empty = report.root.join("receiver");
        std::fs::create_dir(&empty)?;
        let leaf = self.store.create_leaf()?;
        self.store.bind_tree(leaf, &empty, Version::new(0)?)?;
        let start = Instant::now();
        self.store.install(leaf, report.final_version)?;
        report.receiver_install_ms = start.elapsed().as_secs_f64() * 1000.0;
        if fixture::inventory(&empty)? != expected_paths {
            return Err("receiver path set differs from oracle".into());
        }
        if fixture::snapshot(&empty, self.expected.keys())? != self.expected {
            return Err("fresh installed receiver differs from oracle".into());
        }
        report.verified_files = self.expected.len() * 2 + source.snapshot.len();
        Ok(())
    }
}
