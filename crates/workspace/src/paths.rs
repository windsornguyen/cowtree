// Copyright (c) 2026 Windsor Nguyen

//! Resolve portable manifest names without following a symlink parent.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use unicode_casefold::UnicodeCaseFold;
use unicode_normalization::UnicodeNormalization;

use crate::{Error, Result, error::Issue};

pub(crate) fn local(root: &Path, relative: &str) -> Result<PathBuf> {
    if relative.contains('\0')
        || relative.split('/').any(|part| {
            matches!(part, "" | "." | "..")
                || part.eq_ignore_ascii_case(".git")
                || part.eq_ignore_ascii_case(".cowtree")
        })
    {
        return Err(Issue::InvalidPath.at(Path::new(relative)));
    }
    let target = root.join(relative);
    for parent in target.ancestors().skip(1).take_while(|parent| *parent != root) {
        match fs::symlink_metadata(parent) {
            Ok(metadata) if metadata.is_symlink() => return Err(Issue::InvalidPath.at(parent)),
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) => {}
            Err(error) => return Err(Error::io(parent, error)),
        }
    }
    Ok(target)
}

pub(crate) fn aliases<'a>(paths: impl IntoIterator<Item = &'a str>) -> Result<()> {
    let mut spellings = BTreeMap::new();
    for path in paths {
        let mut prefix = String::new();
        for part in path.split('/') {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            let key: String = prefix.nfc().case_fold().collect();
            if spellings.insert(key, prefix.clone()).is_some_and(|previous| previous != prefix) {
                return Err(Issue::AliasedPath.at(Path::new(&prefix)));
            }
        }
    }
    Ok(())
}

pub(crate) fn entries(path: &Path) -> Result<Vec<PathBuf>> {
    let mut entries = fs::read_dir(path)
        .map_err(|error| Error::io(path, error))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|error| Error::io(path, error))?;
    entries.sort();
    Ok(entries)
}

pub(crate) fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(Error::io(path, error)),
    }
}

pub(crate) fn utf8(bytes: &[u8], path: &Path) -> Result<String> {
    String::from_utf8(bytes.into()).map_err(|_| Issue::NonUtf8.at(path))
}
