// Copyright (c) 2026 Windsor Nguyen

//! Bounded file input for clients that already own quiescent filesystem captures.

use crate::{Error, LeafId, LimitKind, Result, Store, objects::ObjectId};
use std::{fs, io::Read, path::Path};

impl Store {
    /// Stage one regular file without encoding its bytes as a JSON integer array.
    pub fn stage_file(&mut self, leaf: LeafId, path: &Path) -> Result<ObjectId> {
        let metadata =
            fs::symlink_metadata(path).map_err(|source| Error::Io { path: path.into(), source })?;
        if !path.is_absolute() || !metadata.is_file() {
            return Err(Error::InvalidInputFile(path.into()));
        }
        if metadata.len() > self.limits.max_object_bytes {
            return Err(Error::Limit(LimitKind::ObjectBytes));
        }
        let file =
            fs::File::open(path).map_err(|source| Error::Io { path: path.into(), source })?;
        let limit = self.limits.max_object_bytes.checked_add(1).ok_or(Error::CounterExhausted)?;
        let mut bytes = Vec::new();
        file.take(limit)
            .read_to_end(&mut bytes)
            .map_err(|source| Error::Io { path: path.into(), source })?;
        if bytes.len() as u64 > self.limits.max_object_bytes {
            return Err(Error::Limit(LimitKind::ObjectBytes));
        }
        self.stage(leaf, &bytes)
    }
}
