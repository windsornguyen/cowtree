// Copyright (c) 2026 Windsor Nguyen
//! Compare file writeout plus one device flush with per-file device flushes.
//! This macOS probe times 64 copied frozen payloads, verifies every byte, and
//! excludes source reads and setup. Successful calls are not power-cut evidence.

#[cfg(target_os = "macos")]
mod probe {
    use cowtree_metadata::objects::ObjectId;
    use serde::Serialize;
    use std::{
        fs::{self, File},
        io::{self, Write},
        path::{Path, PathBuf},
        time::{Duration, Instant},
    };

    #[derive(Clone, Copy, Serialize)]
    #[serde(rename_all = "snake_case")]
    enum Strategy {
        LegacyDouble,
        PerFileFull,
        GroupFull,
    }

    impl Strategy {
        fn file(self, file: &File) -> io::Result<()> {
            match self {
                Self::LegacyDouble => {
                    file.sync_all()?;
                    rustix::fs::fcntl_fullfsync(file)?;
                }
                Self::PerFileFull => rustix::fs::fcntl_fullfsync(file)?,
                Self::GroupFull => rustix::fs::fsync(file)?,
            }
            Ok(())
        }
    }

    #[derive(Serialize)]
    struct Sample {
        strategy: Strategy,
        repeat: usize,
        source_digest: ObjectId,
        files: usize,
        bytes: usize,
        full_barriers: usize,
        verified_files: usize,
        total_ms: f64,
        file_sync_ms: f64,
        directory_sync_ms: f64,
    }

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let args: Vec<_> = std::env::args_os().skip(1).collect();
        if args.len() != 2 {
            return Err("usage: group_durability EXISTING_DIRECTORY FROZEN_SOURCE".into());
        }
        let root = Path::new(&args[0]);
        let source = Path::new(&args[1]);
        let mut paths = Vec::new();
        collect(source, &mut paths)?;
        paths.sort();
        if paths.len() < 64 {
            return Err("source must contain at least 64 regular files".into());
        }
        let payloads: Vec<Vec<u8>> =
            paths.iter().take(64).map(fs::read).collect::<io::Result<_>>()?;
        let identities: Vec<_> = paths
            .iter()
            .zip(&payloads)
            .map(|(path, bytes)| Ok((path.strip_prefix(source)?, ObjectId::from_bytes(bytes))))
            .collect::<Result<_, std::path::StripPrefixError>>()?;
        let digest = ObjectId::from_bytes(&serde_json::to_vec(&identities)?);
        for repeat in 1..=3 {
            let mut strategies =
                [Strategy::LegacyDouble, Strategy::PerFileFull, Strategy::GroupFull];
            strategies.rotate_left(repeat - 1);
            for strategy in strategies {
                let sample = measure(root, &payloads, digest.clone(), repeat, strategy)?;
                println!("{}", serde_json::to_string(&sample)?);
            }
        }
        Ok(())
    }

    fn collect(root: &Path, paths: &mut Vec<PathBuf>) -> io::Result<()> {
        for entry in fs::read_dir(root)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                collect(&entry.path(), paths)?;
            }
            if kind.is_file() {
                paths.push(entry.path());
            }
        }
        Ok(())
    }

    fn measure(
        root: &Path,
        payloads: &[Vec<u8>],
        source_digest: ObjectId,
        repeat: usize,
        strategy: Strategy,
    ) -> io::Result<Sample> {
        let directory =
            tempfile::Builder::new().prefix("cowtree-group-durability-").tempdir_in(root)?;
        let parent = File::open(root)?;
        rustix::fs::fcntl_fullfsync(&parent)?;
        let mut file_sync = Duration::ZERO;
        let started = Instant::now();
        for (index, bytes) in payloads.iter().enumerate() {
            let mut pending = tempfile::NamedTempFile::new_in(directory.path())?;
            pending.write_all(bytes)?;
            let syncing = Instant::now();
            strategy.file(pending.as_file())?;
            file_sync += syncing.elapsed();
            fs::hard_link(pending.path(), directory.path().join(index.to_string()))?;
            pending.close()?;
        }
        let syncing = Instant::now();
        let descriptor = File::open(directory.path())?;
        strategy.file(&descriptor)?;
        if matches!(strategy, Strategy::GroupFull) {
            rustix::fs::fcntl_fullfsync(&descriptor)?;
        }
        let directory_sync = syncing.elapsed();
        let total = started.elapsed();
        for (index, bytes) in payloads.iter().enumerate() {
            if fs::read(directory.path().join(index.to_string()))? != *bytes {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "group bytes differ"));
            }
        }
        directory.close()?;
        let full_barriers = match strategy {
            Strategy::LegacyDouble => (payloads.len() + 1) * 2,
            Strategy::PerFileFull => payloads.len() + 1,
            Strategy::GroupFull => 1,
        };
        Ok(Sample {
            strategy,
            repeat,
            source_digest,
            files: payloads.len(),
            bytes: payloads.iter().map(Vec::len).sum(),
            full_barriers,
            verified_files: payloads.len(),
            total_ms: total.as_secs_f64() * 1_000.0,
            file_sync_ms: file_sync.as_secs_f64() * 1_000.0,
            directory_sync_ms: directory_sync.as_secs_f64() * 1_000.0,
        })
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "macos")]
    return probe::run();
    #[cfg(not(target_os = "macos"))]
    Err("group_durability requires macOS F_FULLFSYNC".into())
}
