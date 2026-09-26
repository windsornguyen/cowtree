// Copyright (c) 2026 Windsor Nguyen

//! Retain exact models, checksums, logs, and expected counterexamples from pinned TLC.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use anyhow::{Result, bail};
use clap::Args;
use regex::Regex;
use sha2::{Digest, Sha256};

use crate::command;

const JAR: &str = "tla2tools-1.8.0.jar";
const SHA256: &str = "066cd246d87a388dfde0f04c3b506007f4c0cb4708a5b5396f0552a005eb75b5";

#[derive(Args)]
pub struct Arguments {
    /// Existing artifact directory outside the checkout.
    #[arg(long)]
    cache: PathBuf,
    /// Java 11+ executable, resolved through PATH when unqualified.
    #[arg(long, default_value = "java")]
    java: PathBuf,
    /// Positive wall-time budget for each finite model.
    #[arg(long, default_value_t = 180)]
    timeout_seconds: u64,
    /// One named safety or counterexample case, or all cases when omitted.
    #[arg(long)]
    case: Option<String>,
}

struct ModelCase {
    /// Configuration and artifact label.
    name: &'static str,
    /// Executable TLA module selected by this configuration.
    module: &'static str,
    /// Exact invariant expected to fail in a witness or deliberate mutation.
    violation: Option<&'static str>,
}

pub fn run(root: &Path, arguments: Arguments) -> Result<()> {
    if arguments.timeout_seconds == 0 {
        bail!("--timeout-seconds must be positive");
    }
    if arguments.case.as_ref().is_some_and(|name| !CASES.iter().any(|case| case.name == name)) {
        bail!("unknown model case");
    }
    let bytes = fs::read(root.join("tools").join(JAR))?;
    if digest(&bytes) != SHA256 {
        bail!("vendored TLC failed SHA-256 validation");
    }
    fs::create_dir_all(&arguments.cache)?;
    let cache = arguments.cache.canonicalize()?;
    if cache.starts_with(root) {
        bail!("--cache must be outside the checkout");
    }
    let jar = cache.join(JAR);
    if !jar.exists() {
        let mut temporary = tempfile::NamedTempFile::new_in(&cache)?;
        std::io::Write::write_all(&mut temporary, &bytes)?;
        temporary.persist(&jar)?;
    }
    if digest(&fs::read(&jar)?) != SHA256 {
        bail!("cached TLC failed SHA-256 validation");
    }
    let version =
        command::output(Command::new(&arguments.java).arg("-version"), Duration::from_secs(10))?;
    if version.timed_out || !version.output.status.success() {
        bail!("Java version query failed");
    }
    let version = String::from_utf8([version.output.stdout, version.output.stderr].concat())?;
    let pattern = Regex::new(r#"version "(?:1\.)?(\d+)"#)?;
    let major = pattern
        .captures(&version)
        .and_then(|capture| capture.get(1))
        .and_then(|value| value.as_str().parse::<u32>().ok());
    if major.is_none_or(|major| major < 11) {
        bail!("Java 11 or later is required");
    }
    fs::write(cache.join("java-version.txt"), &version)?;
    let checker =
        Checker { root, cache: &cache, jar: &jar, version: &version, arguments: &arguments };
    for case in CASES {
        if arguments.case.as_ref().is_none_or(|name| name == case.name) {
            checker.check(case)?;
        }
    }
    Ok(())
}

struct Checker<'a> {
    /// Frozen model source directory.
    root: &'a Path,
    /// Output directory retained as evidence.
    cache: &'a Path,
    /// Verified executable checker artifact.
    jar: &'a Path,
    /// Recorded Java identity.
    version: &'a str,
    /// Execution limits selected by the caller.
    arguments: &'a Arguments,
}

impl Checker<'_> {
    fn check(&self, case: &ModelCase) -> Result<()> {
        let Self { root, cache, jar, version, arguments } = self;
        let directory =
            tempfile::Builder::new().prefix(&format!("{}-", case.name)).tempdir_in(cache)?.keep();
        let config = format!("{}.cfg", case.name);
        let module = format!("{}.tla", case.module);
        let mut files = vec![config.clone(), module.clone()];
        match case.module {
            "FencedPublishInduction" => files.push("FencedPublish.tla".into()),
            "EpochLogReplay" => files.push("EpochLog.tla".into()),
            _ => {}
        }
        let mut log = format!("jar SHA-256: {SHA256}\n{version}");
        for name in files {
            let bytes = fs::read(root.join("specs").join(&name))?;
            fs::write(directory.join(&name), &bytes)?;
            log.push_str(&format!("{name} SHA-256: {}\n", digest(&bytes)));
        }
        let mut command = Command::new(&arguments.java);
        command
            .args(["-Xmx1g", "-XX:+UseParallelGC", "-cp"])
            .arg(jar)
            .args([
                "tlc2.TLC",
                "-workers",
                "1",
                "-fp",
                "0",
                "-coverage",
                "1",
                "-noGenerateSpecTE",
                "-dumpTrace",
                "json",
                "counterexample.json",
                "-config",
                &config,
                &module,
            ])
            .current_dir(&directory)
            .env("TLA2TOOLS_JAR", jar);
        log.push_str(&format!("command: {command:?}\n"));
        fs::write(directory.join("tlc.log"), &log)?;
        let execution =
            command::output(&mut command, Duration::from_secs(arguments.timeout_seconds))?;
        log.push_str(&String::from_utf8_lossy(&execution.output.stdout));
        log.push_str(&String::from_utf8_lossy(&execution.output.stderr));
        log.push_str(&format!("\nexit status: {}\n", execution.output.status));
        if execution.timed_out {
            log.push_str("wall-time limit reached\n");
        }
        fs::write(directory.join("tlc.log"), &log)?;
        if execution.timed_out {
            bail!("{}: wall-time limit reached; {}", case.name, directory.display());
        }
        verdict(
            case,
            &log,
            execution.output.status.code(),
            directory.join("counterexample.json").is_file(),
        )?;
        println!("{}: passed ({})", case.name, directory.join("tlc.log").display());
        Ok(())
    }
}

fn verdict(case: &ModelCase, log: &str, code: Option<i32>, trace: bool) -> Result<()> {
    match case.violation {
        None if code == Some(0)
            && log.contains("Model checking completed. No error has been found.") =>
        {
            Ok(())
        }
        Some(violation)
            if code == Some(12)
                && log.contains(&format!("Invariant {violation} is violated."))
                && log.contains("State 1:")
                && trace =>
        {
            Ok(())
        }
        _ => bail!("{}: checker did not establish the required verdict", case.name),
    }
}

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

const CASES: &[ModelCase] = &[
    ModelCase { name: "PublicationRecovery", module: "PublicationRecovery", violation: None },
    ModelCase {
        name: "PublicationWrongValidation",
        module: "PublicationRecovery",
        violation: Some("ValidateBoundToId"),
    },
    ModelCase {
        name: "PublicationPrematureGC",
        module: "PublicationRecovery",
        violation: Some("AckedRecoverable"),
    },
    ModelCase { name: "FencedPublish", module: "FencedPublish", violation: None },
    ModelCase { name: "FencedPublishExpiry", module: "FencedPublish", violation: None },
    ModelCase { name: "FencedPublishInduction", module: "FencedPublishInduction", violation: None },
    ModelCase {
        name: "FencedPublishInductionTwoWriters",
        module: "FencedPublishInduction",
        violation: None,
    },
    ModelCase {
        name: "MissingFence",
        module: "FencedPublish",
        violation: Some("PublishedWithAuthority"),
    },
    ModelCase {
        name: "WholeLeaf",
        module: "FencedPublish",
        violation: Some("UnmodifiedPathsPreserved"),
    },
    ModelCase {
        name: "StaleBaseWitness",
        module: "FencedPublish",
        violation: Some("NeverAcceptsStaleBase"),
    },
    ModelCase {
        name: "EpochLogReplayDiscard",
        module: "EpochLogReplay",
        violation: Some("ReplayIncomplete"),
    },
    ModelCase {
        name: "EpochLogReplayCommit",
        module: "EpochLogReplay",
        violation: Some("ReplayIncomplete"),
    },
    ModelCase { name: "Workspace", module: "Workspace", violation: None },
    ModelCase { name: "StaleBase", module: "Workspace", violation: Some("NoStaleBaseCommit") },
    ModelCase { name: "StaleToken", module: "Workspace", violation: Some("NoStaleTokenRejection") },
    ModelCase { name: "BrokenFence", module: "Workspace", violation: Some("AcceptedAuthority") },
    ModelCase { name: "Core", module: "Core", violation: None },
    ModelCase { name: "CoreInductive1", module: "Core", violation: None },
    ModelCase { name: "CoreInductive2", module: "Core", violation: None },
    ModelCase { name: "EpochLogTiny", module: "EpochLog", violation: None },
    ModelCase { name: "EpochLogPending", module: "EpochLog", violation: None },
    ModelCase { name: "EpochLogWitness", module: "EpochLog", violation: Some("NoStaleBaseCommit") },
    ModelCase { name: "CowTreeSmoke", module: "CowTree", violation: None },
    ModelCase { name: "CowTreeInductive2", module: "CowTree", violation: None },
];

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_witness_requires_the_expected_invariant_and_retained_trace() {
        let case = ModelCase { name: "witness", module: "Workspace", violation: Some("Fence") };
        let log = "Invariant Fence is violated.\nState 1:";
        assert!(verdict(&case, log, Some(12), true).is_ok());
        assert!(verdict(&case, log, Some(0), true).is_err());
        assert!(verdict(&case, log, Some(12), false).is_err());
        assert!(verdict(&case, "Invariant Other is violated.\nState 1:", Some(12), true).is_err());
    }
}
