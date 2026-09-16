// Copyright (c) 2026 Windsor Nguyen

//! Deterministic source data and a filesystem oracle independent of metadata reads.

use super::config::{Config, Result, random};
use cowtree_metadata::{Entry, EntryKind, ResourcePath, Snapshot, objects::ObjectId};
use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    os::unix::fs::{MetadataExt, symlink},
    path::{Path, PathBuf},
    process::Command,
};

pub struct Source {
    pub root: PathBuf,
    pub snapshot: Snapshot,
    pub editable: Vec<ResourcePath>,
    pub revision: Option<String>,
}

pub fn source(config: &Config) -> Result<Source> {
    let (root, paths) = if let Some(source) = &config.source {
        let root = fs::canonicalize(source)?;
        let output = Command::new("git").arg("-C").arg(&root).args(["ls-files", "-z"]).output()?;
        if !output.status.success() {
            return Err("git ls-files failed".into());
        }
        let clean = Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["diff", "--quiet", "HEAD", "--"])
            .status()?;
        if !clean.success() {
            return Err("Git source must have clean tracked files".into());
        }
        let paths = output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|part| !part.is_empty())
            .map(|part| ResourcePath::parse(std::str::from_utf8(part)?).map_err(Into::into))
            .collect::<Result<Vec<_>>>()?;
        (root, paths)
    } else {
        let root = config.root.join("source");
        fs::create_dir(&root)?;
        let mut state = config.seed;
        let mut paths = Vec::new();
        for index in 0..config.files {
            let path = ResourcePath::parse(format!("file-{index:06}.bin"))?;
            let mut bytes = [0; 4096];
            for chunk in bytes.chunks_exact_mut(8) {
                chunk.copy_from_slice(&random(&mut state).to_le_bytes());
            }
            fs::write(root.join(path.as_str()), bytes)?;
            paths.push(path);
        }
        (root, paths)
    };
    if paths.len() > 100_000 {
        return Err("source exceeds 100000 tracked paths".into());
    }
    let total_bytes = paths.iter().try_fold(0_u64, |total, path| -> Result<u64> {
        Ok(total + fs::symlink_metadata(root.join(path.as_str()))?.len())
    })?;
    if total_bytes > 4 * 1024 * 1024 * 1024 {
        return Err("source exceeds 4 GiB tracked bytes".into());
    }
    let snapshot = snapshot(&root, paths.iter())?;
    if snapshot.len() != paths.len() {
        return Err("duplicate tracked paths".into());
    }
    let revision = if config.source.is_some() { Some(revision(&root)?) } else { None };
    let editable = snapshot
        .iter()
        .filter(|(_, entry)| entry.kind != EntryKind::Symlink)
        .map(|(path, _)| path.clone())
        .collect::<Vec<_>>();
    if editable.len() < 4 {
        return Err("source must contain four regular files".into());
    }
    Ok(Source { root, snapshot, editable, revision })
}

pub fn entry(root: &Path, path: &ResourcePath) -> Result<Entry> {
    let file = root.join(path.as_str());
    let metadata = fs::symlink_metadata(&file)?;
    if metadata.len() > 64 * 1024 * 1024 {
        return Err("source file exceeds 64 MiB object limit".into());
    }
    let (kind, bytes) = if metadata.is_symlink() {
        use std::os::unix::ffi::OsStrExt;
        (EntryKind::Symlink, fs::read_link(file)?.as_os_str().as_bytes().to_vec())
    } else if metadata.is_file() {
        let kind =
            if metadata.mode() & 0o111 != 0 { EntryKind::Executable } else { EntryKind::File };
        (kind, fs::read(file)?)
    } else {
        return Err("tracked source entry is not a regular file or symlink".into());
    };
    Ok(Entry { kind, object: ObjectId::from_bytes(&bytes) })
}

pub fn snapshot<'a>(
    root: &Path,
    paths: impl Iterator<Item = &'a ResourcePath>,
) -> Result<Snapshot> {
    paths.map(|path| Ok((path.clone(), entry(root, path)?))).collect()
}

pub fn clone_tree(source: &Source, target: &Path) -> Result<()> {
    fs::create_dir(target)?;
    for (path, entry) in &source.snapshot {
        let from = source.root.join(path.as_str());
        let to = target.join(path.as_str());
        fs::create_dir_all(to.parent().ok_or("missing target parent")?)?;
        match entry.kind {
            EntryKind::Symlink => symlink(fs::read_link(from)?, to)?,
            EntryKind::File | EntryKind::Executable => {
                reflink_copy::reflink(&from, &to)?;
                fs::set_permissions(to, fs::metadata(from)?.permissions())?;
            }
        }
    }
    Ok(())
}

pub fn modify(root: &Path, path: &ResourcePath, marker: &str) -> Result<Entry> {
    let target = root.join(path.as_str());
    let mut bytes = fs::read(&target)?;
    bytes.extend_from_slice(marker.as_bytes());
    let mut temporary = tempfile::NamedTempFile::new_in(target.parent().ok_or("missing parent")?)?;
    temporary.as_file().set_permissions(fs::metadata(&target)?.permissions())?;
    temporary.write_all(&bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(&target)?;
    entry(root, path)
}

pub fn revision(root: &Path) -> Result<String> {
    let output = Command::new("git").arg("-C").arg(root).args(["rev-parse", "HEAD"]).output()?;
    if !output.status.success() {
        return Err("git rev-parse failed".into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

pub fn inventory(root: &Path) -> Result<BTreeSet<ResourcePath>> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = BTreeSet::new();
    while let Some(directory) = pending.pop() {
        for item in fs::read_dir(directory)? {
            let item = item?;
            let path = item.path();
            if item.file_type()?.is_dir() {
                pending.push(path);
            } else {
                files.insert(ResourcePath::parse(
                    path.strip_prefix(root)?.to_str().ok_or("non-UTF8 filename")?,
                )?);
            }
        }
    }
    Ok(files)
}
