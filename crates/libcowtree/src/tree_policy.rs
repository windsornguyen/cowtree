// Copyright (c) 2026 Windsor Nguyen

//! Compile path policy once before traversing a source or cache tree.

use crate::{Error, Result};
use serde::Serialize;
use std::ffi::{OsStr, OsString};
use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PathClass {
    Source,
    Derived,
    Ephemeral,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureMode {
    Content,
    Metadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Hardlinks {
    #[default]
    Reject,
    Clone,
}

#[derive(Debug, Clone, Default)]
pub struct TreePolicy {
    derived: Vec<Vec<u8>>,
    ephemeral: Vec<Vec<u8>>,
    ignored: Vec<Vec<u8>>,
    hardlinks: Hardlinks,
}

impl TreePolicy {
    pub fn new(
        derived: &[OsString],
        ephemeral: &[OsString],
        ignored: &[OsString],
        hardlinks: Hardlinks,
    ) -> Result<Self> {
        let derived = prefixes(derived)?;
        let ephemeral = prefixes(ephemeral)?;
        let ignored = prefixes(ignored)?;
        let mut selected: Vec<_> = derived.iter().chain(&ephemeral).collect();
        selected.sort_by(|left, right| {
            left.split(|byte| *byte == b'/').cmp(right.split(|byte| *byte == b'/'))
        });
        for pair in selected.windows(2) {
            if within(pair[1], pair[0]) {
                return Err(Error::PolicyOverlap);
            }
        }
        Ok(Self { derived, ephemeral, ignored, hardlinks })
    }

    #[must_use]
    pub fn classify(&self, path: &OsStr) -> PathClass {
        let path = normalized(path);
        if path.split(|byte| *byte == b'/').any(control) {
            return PathClass::Ephemeral;
        }
        if self.ephemeral.iter().any(|prefix| within(&path, prefix)) {
            return PathClass::Ephemeral;
        }
        if self.derived.iter().any(|prefix| within(&path, prefix)) {
            return PathClass::Derived;
        }
        if self.ignored.iter().any(|prefix| within(&path, prefix)) {
            return PathClass::Ephemeral;
        }
        PathClass::Source
    }

    #[must_use]
    pub fn hardlinks(&self) -> Hardlinks {
        self.hardlinks
    }

    #[cfg(unix)]
    pub(crate) fn has_selected_child(&self, path: &OsStr) -> bool {
        let path = normalized(path);
        self.derived.iter().any(|prefix| prefix.len() > path.len() && within(prefix, &path))
    }
}

fn prefixes(values: &[OsString]) -> Result<Vec<Vec<u8>>> {
    values
        .iter()
        .map(|value| {
            let key = normalized(value);
            if key.contains(&0)
                || key
                    .split(|byte| *byte == b'/')
                    .any(|part| part.is_empty() || part == b"." || part == b".." || control(part))
            {
                return Err(Error::InvalidPolicy { path: value.into() });
            }
            Ok(key)
        })
        .collect()
}

fn normalized(value: &OsStr) -> Vec<u8> {
    let mut output = Vec::new();
    for chunk in value.as_encoded_bytes().utf8_chunks() {
        output.extend(chunk.valid().nfc().collect::<String>().as_bytes());
        output.extend(chunk.invalid());
    }
    output
}

fn control(part: &[u8]) -> bool {
    part.eq_ignore_ascii_case(b".git") || part.eq_ignore_ascii_case(b".cowtree")
}

fn within(path: &[u8], prefix: &[u8]) -> bool {
    path == prefix || path.strip_prefix(prefix).is_some_and(|suffix| suffix.first() == Some(&b'/'))
}
