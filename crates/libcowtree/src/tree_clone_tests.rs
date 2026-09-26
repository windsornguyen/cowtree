// Copyright (c) 2026 Windsor Nguyen

//! Mutation at clone boundaries cannot become a completed content or metadata capture.

use super::{clone_with, populate_with};
use crate::{CaptureMode, Error, Operation, TreePolicy};
use std::{error::Error as StdError, fs, io, path::Path};

type TestResult = Result<(), Box<dyn StdError>>;

fn supported(path: &Path) -> Result<bool, Box<dyn StdError>> {
    let report = crate::inspect_path(path)?;
    if std::env::var_os("COWTREE_EXPECT_SUPPORTED").is_some_and(|value| value == "1") {
        assert!(report.supported());
    }
    if !report.supported() {
        eprintln!("skip native filesystem: {:?}", report.reason);
    }
    Ok(report.supported())
}

fn corrupt(path: &Path) -> io::Result<()> {
    let metadata = fs::metadata(path)?;
    fs::write(path, b"changed\n")?;
    let output = fs::File::open(path)?;
    output.set_times(fs::FileTimes::new().set_modified(metadata.modified()?))
}

#[test]
fn corrupted_clone_never_becomes_a_completed_tree() -> TestResult {
    let directory = tempfile::tempdir()?;
    if !supported(directory.path())? {
        return Ok(());
    }
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    fs::create_dir(&source)?;
    fs::write(source.join("file"), b"original")?;
    let clone = |source: &Path, target: &Path| {
        crate::clone_file(source, target)?;
        corrupt(target).map_err(|error| Error::io(Operation::Metadata, target, error))
    };
    let result = clone_with(&source, &target, &TreePolicy::default(), clone);
    assert!(matches!(result, Err(Error::SourceChanged { .. })));
    assert!(!target.exists());
    assert_eq!(fs::read(source.join("file"))?, b"original");
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum Mutation {
    CopiedFile,
    PendingFile,
    ParentLink,
    ExtraFile,
    MissingFile,
}

fn mutate(root: &Path, outside: &Path, mutation: Mutation) -> io::Result<()> {
    match mutation {
        Mutation::CopiedFile => corrupt(&root.join("a")),
        Mutation::PendingFile => corrupt(&root.join("nested/file")),
        Mutation::ParentLink => {
            fs::remove_file(root.join("nested/file"))?;
            fs::remove_dir(root.join("nested"))?;
            std::os::unix::fs::symlink(outside, root.join("nested"))
        }
        Mutation::ExtraFile => fs::write(root.join("extra"), b"unpublished"),
        Mutation::MissingFile => fs::remove_file(root.join("a")),
    }
}

#[test]
fn source_changes_refuse_content_and_metadata_captures() -> TestResult {
    for capture in [CaptureMode::Content, CaptureMode::Metadata] {
        for mutation in [
            Mutation::CopiedFile,
            Mutation::PendingFile,
            Mutation::ParentLink,
            Mutation::ExtraFile,
            Mutation::MissingFile,
        ] {
            let directory = tempfile::tempdir()?;
            if !supported(directory.path())? {
                return Ok(());
            }
            let source = directory.path().join("source");
            let target = directory.path().join("target");
            let outside = directory.path().join("outside");
            fs::create_dir_all(source.join("nested"))?;
            fs::create_dir(&target)?;
            fs::create_dir(&outside)?;
            fs::write(source.join("a"), b"original")?;
            fs::write(source.join("nested/file"), b"original")?;
            fs::write(outside.join("file"), b"original")?;
            let clone = |file: &Path, target: &Path| {
                crate::clone_file(file, target)?;
                if file == source.join("a") {
                    mutate(&source, &outside, mutation)
                        .map_err(|error| Error::io(Operation::Metadata, file, error))?;
                }
                Ok(())
            };
            let result = populate_with(&source, &target, &TreePolicy::default(), capture, clone);
            assert!(result.is_err(), "{capture:?}, {mutation:?} accepted a changed source");
            assert_eq!(fs::read(outside.join("file"))?, b"original");
        }
    }
    Ok(())
}
