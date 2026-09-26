// Copyright (c) 2026 Windsor Nguyen

//! Bind explicit source/cache rules to one checkout's tracked and ignored paths.

use std::{collections::BTreeSet, ffi::OsString, path::Path};

use cowtree::{PathClass, TreePolicy};

use crate::{Policy, Result, error::Issue, paths, types::Hardlinks};

impl Policy {
    pub(crate) fn compile(&self) -> Result<TreePolicy> {
        let derived: Vec<_> = self.derived.iter().map(OsString::from).collect();
        let ephemeral: Vec<_> = self.ephemeral.iter().map(OsString::from).collect();
        let ignored: Vec<_> = self.ignored.iter().map(OsString::from).collect();
        let hardlinks = match self.derived_hardlinks {
            Hardlinks::Reject => cowtree::Hardlinks::Reject,
            Hardlinks::Clone => cowtree::Hardlinks::Clone,
        };
        Ok(TreePolicy::new(&derived, &ephemeral, &ignored, hardlinks)?)
    }

    pub(crate) fn working(&self, source: &Path) -> Result<Self> {
        let policy = self.compile()?;
        let repository = cowtree_git::Client::discover(Some(source))?;
        if repository.root != source {
            return Err(Issue::SourceRoot.at(source));
        }
        let sparse = repository.output(repository.command()?.args([
            "config",
            "--bool",
            "core.sparseCheckout",
        ]))?;
        if sparse.status.success() && sparse.stdout == b"true\n" {
            return Err(Issue::SparseCheckout.at(source));
        }
        if !matches!(sparse.status.code(), Some(0 | 1)) {
            return Err(cowtree_git::Error::Command {
                status: sparse.status,
                stderr: String::from_utf8_lossy(&sparse.stderr).into_owned(),
            }
            .into());
        }
        let tracked = repository.capture(repository.command()?.args(["ls-files", "-z"]))?;
        let tracked = paths::utf8(&tracked, source)?;
        let mut names = BTreeSet::new();
        for name in tracked.split('\0').filter(|name| !name.is_empty()) {
            if policy.classify(name.as_ref()) != PathClass::Source {
                return Err(Issue::ConflictingPolicy.at(&source.join(name)));
            }
            names.insert(name.to_owned());
        }
        let ignored = repository.capture(repository.command()?.args([
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
            "-z",
        ]))?;
        let ignored = paths::utf8(&ignored, source)?;
        let mut selected = self.clone();
        for name in ignored.split('\0').filter(|name| !name.is_empty()) {
            if name.split('/').any(|part| {
                part.eq_ignore_ascii_case(".git") || part.eq_ignore_ascii_case(".cowtree")
            }) {
                continue;
            }
            let name = name.trim_end_matches('/');
            selected.ignored.push(name.to_owned());
            names.insert(name.to_owned());
        }
        names.extend(self.derived.iter().cloned());
        names.extend(self.ephemeral.iter().cloned());
        paths::aliases(names.iter().map(String::as_str))?;
        Ok(selected)
    }
}
