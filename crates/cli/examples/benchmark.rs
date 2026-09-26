// Copyright (c) 2026 Windsor Nguyen

//! Compare complete native creation with Git using one verified synthetic fixture.

use std::{
    error::Error,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

use clap::Parser;
use serde::Serialize;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Parser)]
struct Arguments {
    /// Native executable whose complete startup and operation are timed.
    #[arg(long)]
    binary: PathBuf,
    /// JSON evidence destination outside the checkout.
    #[arg(long)]
    output: PathBuf,
    /// Tracked files in each worktree.
    #[arg(long, default_value_t = 512)]
    files: usize,
    /// Logical bytes in each deterministic, compressible file.
    #[arg(long, default_value_t = 8192)]
    bytes_per_file: usize,
    /// Repeated pairs with alternating method order.
    #[arg(long, default_value_t = 5)]
    trials: usize,
    /// Cowtree population mode compared with Git checkout.
    #[arg(long, value_enum, default_value_t = Source::Checkout)]
    source_mode: Source,
}

#[derive(Clone, Copy, clap::ValueEnum, Serialize)]
#[serde(rename_all = "snake_case")]
enum Source {
    Checkout,
    Committed,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum Method {
    Git,
    Cowtree,
}

#[derive(Serialize)]
struct Sample {
    /// Complete creation path exercised by this measurement.
    method: Method,
    /// Zero-based paired trial.
    trial: usize,
    /// Wall time including executable startup, admission, cloning, and verification.
    seconds: f64,
}

#[derive(Serialize)]
struct Report {
    /// Operating system hosting the experiment.
    platform: &'static str,
    /// Native CPU architecture.
    architecture: &'static str,
    /// Git distribution used by both methods.
    git: String,
    /// Tracked entries per creation.
    files: usize,
    /// Logical bytes per file, not physical allocation.
    bytes_per_file: usize,
    /// Explicit Cowtree creation mode under test.
    source_mode: Source,
    /// Raw paired samples, retained without dropping outliers.
    samples: Vec<Sample>,
}

fn main() -> Result<()> {
    let arguments = Arguments::parse();
    if !(1..=20000).contains(&arguments.files)
        || !(1..=9).contains(&arguments.trials)
        || !(1..=1048576).contains(&arguments.bytes_per_file)
    {
        return Err("require 1..20000 files, 1..9 trials, and 1..1048576 bytes per file".into());
    }
    let root = tempfile::Builder::new().prefix("cowtree-benchmark-").tempdir()?.keep();
    match measure(&root, &arguments) {
        Ok(report) => {
            fs::write(&arguments.output, serde_json::to_vec_pretty(&report)?)?;
            fs::remove_dir_all(&root)?;
            println!("{}", arguments.output.display());
            Ok(())
        }
        Err(error) => Err(format!("{error}; retained fixture: {}", root.display()).into()),
    }
}

fn measure(root: &Path, arguments: &Arguments) -> Result<Report> {
    let source = root.join("source");
    prepare(&source, arguments)?;
    let binary = arguments.binary.canonicalize()?;
    let mut samples = Vec::new();
    for trial in 0..arguments.trials {
        let methods = if trial % 2 == 0 {
            [Method::Git, Method::Cowtree]
        } else {
            [Method::Cowtree, Method::Git]
        };
        for method in methods {
            let target = root.join("target");
            let started = Instant::now();
            match method {
                Method::Git => {
                    command(
                        &source,
                        "git",
                        [
                            "worktree".into(),
                            "add".into(),
                            "--detach".into(),
                            target.clone().into_os_string(),
                        ],
                    )?;
                }
                Method::Cowtree => {
                    let mut args =
                        vec!["add".into(), "--detach".into(), target.clone().into_os_string()];
                    if matches!(arguments.source_mode, Source::Committed) {
                        args.insert(1, "--committed".into());
                    }
                    command(&source, &binary, args)?;
                }
            }
            samples.push(Sample { method, trial, seconds: started.elapsed().as_secs_f64() });
            for index in 0..arguments.files {
                if fs::read(source.join(relative(index)))?
                    != fs::read(target.join(relative(index)))?
                {
                    return Err("destination bytes differ from fixture".into());
                }
            }
            if !command(&target, "git", ["status".into(), "--porcelain".into()])?.is_empty() {
                return Err("destination is not clean".into());
            }
            command(&source, "git", ["worktree".into(), "remove".into(), target.into_os_string()])?;
        }
    }
    let git = String::from_utf8(command(&source, "git", ["--version".into()])?)?;
    Ok(Report {
        platform: std::env::consts::OS,
        architecture: std::env::consts::ARCH,
        git: git.trim().into(),
        files: arguments.files,
        bytes_per_file: arguments.bytes_per_file,
        source_mode: arguments.source_mode,
        samples,
    })
}

fn relative(index: usize) -> PathBuf {
    PathBuf::from(format!("files/{:04}/{index:06}.dat", index / 256))
}

fn command(
    root: &Path,
    program: impl AsRef<std::ffi::OsStr>,
    args: impl IntoIterator<Item = OsString>,
) -> Result<Vec<u8>> {
    let output = Command::new(program).args(args).current_dir(root).output()?;
    if !output.status.success() {
        return Err(
            format!("{}: {}", output.status, String::from_utf8_lossy(&output.stderr)).into()
        );
    }
    Ok(output.stdout)
}

fn prepare(source: &Path, arguments: &Arguments) -> Result<()> {
    fs::create_dir(source)?;
    for index in 0..arguments.files {
        let path = source.join(relative(index));
        fs::create_dir_all(path.parent().ok_or("fixture path has no parent")?)?;
        fs::write(path, vec![(index % 251) as u8; arguments.bytes_per_file])?;
    }
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.name", "Benchmark"],
        vec!["config", "user.email", "bench@example.invalid"],
        vec!["config", "core.autocrlf", "false"],
        vec!["add", "."],
        vec!["-c", "commit.gpgsign=false", "commit", "-qm", "fixture"],
    ] {
        command(source, "git", args.into_iter().map(OsString::from))?;
    }
    Ok(())
}
