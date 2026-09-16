// Copyright (c) 2026 Windsor Nguyen

//! Reproducible storage and contention qualification with a checked filesystem oracle.

#[path = "qualify/config.rs"]
mod config;
#[path = "qualify/fixture.rs"]
mod fixture;
#[path = "qualify/measure.rs"]
mod measure;
#[path = "qualify/workload.rs"]
mod workload;

use config::{Config, Result};
use serde::Serialize;

#[derive(Serialize)]
struct Report {
    schema: u32,
    config: Config,
    sqlite: String,
    revision: String,
    executable_sha256: String,
    source_manifest_sha256: String,
    source_revision: Option<String>,
    trials: Vec<workload::Trial>,
    median_batches_per_second: f64,
    allocation_note: &'static str,
}

fn main() -> Result<()> {
    let config = Config::parse()?;
    let executable_sha256 =
        cowtree_metadata::objects::ObjectId::from_bytes(&std::fs::read(std::env::current_exe()?)?)
            .to_string();
    let revision = fixture::revision(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))?;
    std::fs::create_dir(&config.root)?;
    let source = fixture::source(&config)?;
    let mut trials = Vec::new();
    for trial in 0..config.trials {
        eprintln!("trial {}/{}: {} files", trial + 1, config.trials, source.snapshot.len());
        trials.push(workload::run(&config, &source, trial)?);
    }
    let mut rates: Vec<_> = trials.iter().map(|trial| trial.batches_per_second).collect();
    rates.sort_by(f64::total_cmp);
    let median = if rates.len() % 2 == 0 {
        (rates[rates.len() / 2 - 1] + rates[rates.len() / 2]) / 2.0
    } else {
        rates[rates.len() / 2]
    };
    let report = Report {
        schema: 1,
        config,
        sqlite: rusqlite::version().into(),
        revision,
        executable_sha256,
        source_manifest_sha256: cowtree_metadata::objects::ObjectId::from_bytes(
            &serde_json::to_vec(&source.snapshot)?,
        )
        .to_string(),
        source_revision: source.revision,
        trials,
        median_batches_per_second: median,
        allocation_note: "st_blocks is per-file accounting, not exclusive APFS physical usage; shared blocks must not be summed as space savings",
    };
    serde_json::to_writer_pretty(std::io::stdout().lock(), &report)?;
    Ok(())
}
