// Copyright (c) 2026 Windsor Nguyen

//! Resume bounded source import from a frozen checkpoint before publishing config.

use std::{fs::File, path::Path, sync::Arc};

use cowtree_metadata::{ImportProgress, Store};

use crate::{
    Config, Result, Workspace, error::Issue, git::Repository, nodes::Nodes, paths, records,
};

pub(crate) fn run(staging: &Path, config: &Config, locks: Vec<Arc<File>>) -> Result<()> {
    let repository = Repository::authority(&config.git_directory, locks);
    let nodes =
        Nodes { directory: staging.join("nodes"), repository, policy: config.policy.clone() };
    let node = nodes.read(&config.initial)?;
    nodes.verify(&node)?;
    let mut authority = Store::open(&staging.join("authority"))?;
    let mut progress = authority.begin_import(node.source.clone())?;
    while progress.completed < progress.total {
        let previous = progress.completed;
        progress = authority
            .import_chunk(&progress.root, &nodes.directory.join(node.id.as_str()).join("tree"))?;
        if progress.completed <= previous {
            return Err(Issue::IncompleteRecord.at(staging));
        }
    }
    progress = authority.finish_import(&progress.root)?;
    if !progress.complete || authority.tip()?.1 != progress.root {
        return Err(Issue::IncompleteRecord.at(staging));
    }
    nodes.repository.set_ref(
        &format!("refs/cowtree/tips/{}", node.id.as_str()),
        &node.git_commit,
        None,
    )?;
    records::write(&staging.join("workspace.json"), config)?;
    records::sync_directory(staging)
}

impl Workspace {
    pub fn import_status(root: &Path) -> Result<Option<ImportProgress>> {
        let root = crate::workspace::canonical_target(root)?;
        let (staging, config) = if paths::exists(&root)? {
            let workspace = Self::open(&root)?;
            (root, workspace.config)
        } else {
            let staging = Self::staging(&root)?;
            let config: Config = records::read(&staging.join("import.json"))?;
            if config.location != root {
                return Err(Issue::ChangedDirectory.at(&root));
            }
            (staging, config)
        };
        let _ = config;
        Ok(Store::open(&staging.join("authority"))?.status_import()?)
    }
}
