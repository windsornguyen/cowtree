// Copyright (c) 2026 Windsor Nguyen

//! Compare legacy double flush, std, and Cowtree's single flush on fresh publications.
//! Rotate three strategies over three runs; verify bytes after each timed operation.
//! This measures successful OS calls, not recovery after power loss.

use serde::Serialize;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;
use std::time::{Duration, Instant};

#[path = "../src/durability.rs"]
mod durability;

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum Strategy {
    LegacyDouble,
    Std,
    SingleFull,
}

impl Strategy {
    fn sync_file(self, file: &File) -> io::Result<()> {
        match self {
            Self::LegacyDouble => {
                file.sync_all()?;
                #[cfg(target_os = "macos")]
                rustix::fs::fcntl_fullfsync(file)?;
                Ok(())
            }
            Self::Std => file.sync_all(),
            Self::SingleFull => durability::sync_file(file),
        }
    }
}

#[derive(Serialize)]
struct Sample {
    strategy: Strategy,
    repeat: usize,
    publications: usize,
    bytes_per_file: usize,
    file_barriers: usize,
    directory_barriers: usize,
    verified_files: usize,
    publication_ms: f64,
    file_sync_ms: f64,
    directory_sync_ms: f64,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() != 2 {
        return Err("usage: durability EXISTING_DIRECTORY PUBLICATIONS_PER_SAMPLE (1..4096)".into());
    }
    let root = Path::new(&arguments[0]);
    let count: usize = arguments[1].to_str().ok_or("count must be UTF-8")?.parse()?;
    if !(1..=4096).contains(&count) {
        return Err("count must be between 1 and 4096".into());
    }
    for repeat in 1..=3 {
        let mut strategies = [Strategy::LegacyDouble, Strategy::Std, Strategy::SingleFull];
        strategies.rotate_left(repeat - 1);
        for strategy in strategies {
            let sample = measure(root, count, repeat, strategy)?;
            println!("{}", serde_json::to_string(&sample)?);
        }
    }
    Ok(())
}

fn measure(root: &Path, count: usize, repeat: usize, strategy: Strategy) -> io::Result<Sample> {
    let directory = tempfile::Builder::new().prefix("cowtree-durability-").tempdir_in(root)?;
    let mut publication_time = Duration::ZERO;
    let mut file_sync_time = Duration::ZERO;
    let mut directory_sync_time = Duration::ZERO;
    for index in 0..count {
        let target = directory.path().join(index.to_string());
        let mut bytes = [0x5a; 4096];
        bytes[..8].copy_from_slice(&(index as u64).to_le_bytes());
        let started = Instant::now();
        let mut temporary = tempfile::NamedTempFile::new_in(directory.path())?;
        temporary.write_all(&bytes)?;
        let syncing = Instant::now();
        strategy.sync_file(temporary.as_file())?;
        file_sync_time += syncing.elapsed();
        fs::hard_link(temporary.path(), &target)?;
        temporary.close()?;
        let syncing = Instant::now();
        match strategy {
            Strategy::SingleFull => durability::sync_directory(directory.path())?,
            Strategy::LegacyDouble | Strategy::Std => {
                strategy.sync_file(&File::open(directory.path())?)?;
            }
        }
        directory_sync_time += syncing.elapsed();
        publication_time += started.elapsed();
        if fs::read(&target)? != bytes {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "published bytes differ"));
        }
    }
    directory.close()?;
    let sample = Sample {
        strategy,
        repeat,
        publications: count,
        bytes_per_file: 4096,
        file_barriers: count,
        directory_barriers: count,
        verified_files: count,
        publication_ms: publication_time.as_secs_f64() * 1_000.0,
        file_sync_ms: file_sync_time.as_secs_f64() * 1_000.0,
        directory_sync_ms: directory_sync_time.as_secs_f64() * 1_000.0,
    };
    Ok(sample)
}
