// Copyright (c) 2026 Windsor Nguyen

//! Native clones preserve bytes and metadata while rejecting foreign destinations.

use cowtree::{Error, clone_file};
use std::{error::Error as StdError, fs, io::ErrorKind};

fn native_directory() -> Result<Option<tempfile::TempDir>, Box<dyn StdError>> {
    let directory = tempfile::tempdir()?;
    let report = cowtree::inspect_path(directory.path())?;
    if std::env::var_os("COWTREE_EXPECT_SUPPORTED").is_some_and(|value| value == "1") {
        assert!(report.supported(), "{:?}", report.reason);
    }
    if report.supported() {
        return Ok(Some(directory));
    }
    eprintln!("skip native filesystem: {:?}", report.reason);
    Ok(None)
}

#[test]
fn existing_destination_is_never_modified() -> Result<(), Box<dyn StdError>> {
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    fs::write(&source, b"source")?;
    fs::write(&target, b"owned by someone else")?;
    let error = clone_file(&source, &target);
    assert!(
        matches!(error, Err(Error::Io { source, .. }) if source.kind() == ErrorKind::AlreadyExists)
    );
    assert_eq!(fs::read(&target)?, b"owned by someone else");
    assert_eq!(fs::read(&source)?, b"source");
    Ok(())
}

#[test]
fn directory_source_cannot_create_a_destination() -> Result<(), Box<dyn StdError>> {
    let directory = tempfile::tempdir()?;
    let target = directory.path().join("target");
    assert!(matches!(clone_file(directory.path(), &target), Err(Error::InvalidSource { .. })));
    assert!(!target.exists());
    Ok(())
}

#[test]
fn native_clone_writes_are_independent() -> Result<(), Box<dyn StdError>> {
    let Some(directory) = native_directory()? else {
        return Ok(());
    };
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    for size in [0, 1, 4095, 4096, 4097, 65537] {
        let bytes = vec![b'a'; size];
        fs::write(&source, &bytes)?;
        clone_file(&source, &target)?;
        assert_eq!(fs::read(&target)?, bytes);
        fs::write(&target, b"target edit")?;
        assert_eq!(fs::read(&source)?, bytes);
        fs::write(&source, b"source edit")?;
        assert_eq!(fs::read(&target)?, b"target edit");
        fs::remove_file(&target)?;
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn symbolic_link_source_is_not_followed() -> Result<(), Box<dyn StdError>> {
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("source");
    let link = directory.path().join("link");
    let target = directory.path().join("target");
    fs::write(&source, b"source")?;
    std::os::unix::fs::symlink(&source, &link)?;
    assert!(matches!(clone_file(&link, &target), Err(Error::InvalidSource { .. })));
    assert!(!target.exists());
    Ok(())
}

#[cfg(unix)]
#[test]
fn clone_preserves_permissions_and_timestamps() -> Result<(), Box<dyn StdError>> {
    use std::{
        fs::FileTimes,
        os::unix::fs::PermissionsExt,
        time::{Duration, UNIX_EPOCH},
    };

    let Some(directory) = native_directory()? else {
        return Ok(());
    };
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    fs::write(&source, b"executable")?;
    let file = fs::File::open(&source)?;
    let modified = UNIX_EPOCH + Duration::new(1_600_000_000, 123_456_789);
    file.set_times(FileTimes::new().set_modified(modified).set_accessed(modified))?;
    file.set_permissions(fs::Permissions::from_mode(0o750))?;
    clone_file(&source, &target)?;
    let actual = fs::metadata(&target)?;
    assert_eq!(actual.permissions().mode() & 0o7777, 0o750);
    assert_eq!(actual.modified()?, modified);
    assert_eq!(actual.accessed()?, modified);
    Ok(())
}

#[test]
fn atomic_editor_save_does_not_change_the_original() -> Result<(), Box<dyn StdError>> {
    let Some(directory) = native_directory()? else {
        return Ok(());
    };
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    let temporary = directory.path().join("editor-save");
    fs::write(&source, b"original")?;
    clone_file(&source, &target)?;
    fs::write(&temporary, b"saved edit")?;
    fs::rename(&temporary, &target)?;
    assert_eq!(fs::read(&source)?, b"original");
    assert_eq!(fs::read(&target)?, b"saved edit");
    Ok(())
}

#[test]
fn competing_clones_preserve_the_winning_destination() -> Result<(), Box<dyn StdError>> {
    let Some(directory) = native_directory()? else {
        return Ok(());
    };
    let sources = [directory.path().join("first"), directory.path().join("second")];
    fs::write(&sources[0], b"first source")?;
    fs::write(&sources[1], b"second source")?;
    let target = directory.path().join("target");
    let barrier = std::sync::Barrier::new(2);
    let outcomes = std::thread::scope(|scope| {
        let first = scope.spawn(|| {
            barrier.wait();
            clone_file(&sources[0], &target)
        });
        let second = scope.spawn(|| {
            barrier.wait();
            clone_file(&sources[1], &target)
        });
        [first.join(), second.join()]
    });
    let mut winners = Vec::new();
    for (index, outcome) in outcomes.into_iter().enumerate() {
        match outcome.map_err(|_| "clone worker panicked")? {
            Ok(()) => winners.push(index),
            Err(Error::Io { source, .. }) if source.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    assert_eq!(winners.len(), 1);
    assert_eq!(fs::read(&target)?, fs::read(&sources[winners[0]])?);
    assert_eq!(fs::read(&sources[0])?, b"first source");
    assert_eq!(fs::read(&sources[1])?, b"second source");
    Ok(())
}
