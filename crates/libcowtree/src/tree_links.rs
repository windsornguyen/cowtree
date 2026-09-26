// Copyright (c) 2026 Windsor Nguyen

//! Cache aliases must retain private meaning when the root becomes another clone.
//!
//! Resolve relative link chains beneath the capture root, including missing
//! endpoints. Absolute links, cycles, and hops through non-cache symlinks cannot
//! preserve that ownership and are rejected before a completed tree is returned.

use crate::{Error, Operation, PathClass, Result, TreePolicy};
use std::{collections::VecDeque, ffi::OsString, fs, io, path::Path};

pub(crate) fn validate(root: &Path, path: &Path, link: &Path, policy: &TreePolicy) -> Result<()> {
    let invalid = || Error::DerivedLink { path: path.into() };
    if link.is_absolute() {
        return Err(invalid());
    }
    let root = root.canonicalize().map_err(|error| Error::io(Operation::Inspect, root, error))?;
    let parent = path.parent().ok_or_else(invalid)?;
    let mut pending: VecDeque<OsString> = parent.join(link).iter().map(OsString::from).collect();
    let mut resolved = root.clone();
    let mut links = 0;
    while let Some(component) = pending.pop_front() {
        if component == "." {
            continue;
        }
        if component == ".." {
            if resolved == root {
                return Err(invalid());
            }
            resolved.pop();
            continue;
        }
        resolved.push(component);
        let metadata = match fs::symlink_metadata(&resolved) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(Error::io(Operation::Inspect, &resolved, error)),
        };
        if !metadata.is_symlink() {
            continue;
        }
        let relative = resolved.strip_prefix(&root).map_err(|_| invalid())?;
        if policy.classify(relative.as_os_str()) != PathClass::Derived || links == 40 {
            return Err(invalid());
        }
        links += 1;
        let next = fs::read_link(&resolved)
            .map_err(|error| Error::io(Operation::ReadLink, &resolved, error))?;
        if next.is_absolute() {
            return Err(invalid());
        }
        resolved.pop();
        for component in next.iter().rev() {
            pending.push_front(component.into());
        }
    }
    let relative = resolved.strip_prefix(&root).map_err(|_| invalid())?;
    if policy.classify(relative.as_os_str()) != PathClass::Derived {
        return Err(invalid());
    }
    Ok(())
}
