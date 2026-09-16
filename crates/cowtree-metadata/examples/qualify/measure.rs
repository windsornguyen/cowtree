// Copyright (c) 2026 Windsor Nguyen

//! Bounded latency histograms and independent SQLite connections under contention.

use super::config::Result;
use cowtree_metadata::{ErrorCode, Store};
use serde::Serialize;
use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Default, Serialize)]
pub struct Latency {
    pub count: u64,
    pub total_ms: f64,
    pub max_ms: f64,
    pub histogram_log2_microseconds: Vec<u64>,
}
impl Latency {
    pub fn record(&mut self, elapsed: Duration) {
        self.count += 1;
        let milliseconds = elapsed.as_secs_f64() * 1000.0;
        self.total_ms += milliseconds;
        self.max_ms = self.max_ms.max(milliseconds);
        if self.histogram_log2_microseconds.is_empty() {
            self.histogram_log2_microseconds.resize(64, 0);
        }
        let micros = elapsed.as_micros().min(u128::from(u64::MAX)) as u64;
        let bucket = (64 - micros.leading_zeros()).min(63) as usize;
        self.histogram_log2_microseconds[bucket] += 1;
    }
}
#[derive(Default, Serialize)]
pub struct Reader {
    pub snapshots: Latency,
    pub writer_lock_acquisition: Latency,
    pub busy: u64,
    pub final_observed_version: u64,
}
pub struct Readers {
    stop: Arc<AtomicBool>,
    handles: Vec<JoinHandle<Result<Reader>>>,
}
impl Readers {
    pub fn start(root: &Path, count: usize) -> Result<Self> {
        let mut readers = Self { stop: Arc::new(AtomicBool::new(false)), handles: Vec::new() };
        for index in 0..count {
            let store = Store::open(root)?;
            let probe = rusqlite::Connection::open(root.join("metadata.sqlite3"))?;
            probe.busy_timeout(Duration::from_secs(5))?;
            let stop = readers.stop.clone();
            readers.handles.push(thread::spawn(move || reader(store, probe, stop, index == 0)));
        }
        Ok(readers)
    }
    pub fn finish(mut self) -> Result<Vec<Reader>> {
        self.stop.store(true, Ordering::Release);
        self.handles.drain(..).map(|handle| handle.join().map_err(|_| "reader panicked")?).collect()
    }
}
impl Drop for Readers {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}
fn reader(
    mut store: Store,
    probe: rusqlite::Connection,
    stop: Arc<AtomicBool>,
    lock_probe: bool,
) -> Result<Reader> {
    let mut report = Reader::default();
    loop {
        let start = Instant::now();
        let observed =
            store.tip().and_then(|(version, _)| store.snapshot(version).map(|_| version));
        match observed {
            Ok(version) => {
                if version.get() < report.final_observed_version {
                    return Err("reader tip regressed".into());
                }
                report.final_observed_version = version.get();
                report.snapshots.record(start.elapsed());
            }
            Err(error) if error.code() == ErrorCode::DatabaseBusy => report.busy += 1,
            Err(error) => return Err(error.into()),
        }
        if lock_probe {
            let start = Instant::now();
            match probe.execute_batch("BEGIN IMMEDIATE;") {
                Ok(()) => {
                    report.writer_lock_acquisition.record(start.elapsed());
                    probe.execute_batch("ROLLBACK;")?;
                }
                Err(error)
                    if matches!(
                        error.sqlite_error_code(),
                        Some(
                            rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                        )
                    ) =>
                {
                    report.busy += 1
                }
                Err(error) => return Err(error.into()),
            }
        }
        if stop.load(Ordering::Acquire) {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    Ok(report)
}

#[derive(Default, Serialize)]
pub struct Allocation {
    pub files: u64,
    pub logical_bytes: u64,
    pub st_blocks_bytes: u64,
}
pub fn allocation(root: &Path) -> Result<Allocation> {
    let mut report = Allocation::default();
    for item in fs::read_dir(root)? {
        let item = item?;
        let metadata = fs::symlink_metadata(item.path())?;
        if metadata.is_dir() {
            let nested = allocation(&item.path())?;
            report.files += nested.files;
            report.logical_bytes += nested.logical_bytes;
            report.st_blocks_bytes += nested.st_blocks_bytes;
        } else {
            report.files += 1;
            report.logical_bytes += metadata.len();
            report.st_blocks_bytes += metadata.blocks() * 512;
        }
    }
    Ok(report)
}
pub fn length(path: &Path) -> Result<u64> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(metadata.len()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error.into()),
    }
}
