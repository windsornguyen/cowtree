// Copyright (c) 2026 Windsor Nguyen

//! Create and reopen a workspace whose published records own its filesystem state.

use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    sync::Arc,
};

use cowtree_metadata::{Limits, Store};
use serde::{Deserialize, Serialize};

use crate::{
    Config, Error, Policy, Result, durability,
    error::Issue,
    git::Repository,
    imports,
    nodes::{Nodes, Seal},
    paths, records, submodules,
};

pub(crate) const DIRECTORIES: [&str; 7] =
    ["nodes", "leaves", "operations", "checks", "retained", "trash", "receipts"];

#[derive(Clone, Debug)]
pub struct CreateRequest {
    /// Absent destination outside the source checkout.
    pub root: PathBuf,
    /// Clean parent checkout supplying source and selected caches.
    pub source: PathBuf,
    /// Executable recorded as the initializer, independent of future validation callers.
    pub program: PathBuf,
    /// Explicit source, cache, and dependency policy.
    pub policy: Policy,
}

#[derive(Clone, Debug)]
pub struct Workspace {
    /// Canonical directory whose identity is checked for every transaction.
    pub root: PathBuf,
    /// Durable configuration read when this handle was opened.
    pub config: Config,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Initialization {
    /// Destination exclusively claimed after the import finishes.
    pub target: PathBuf,
    /// Original source, retained for diagnostics.
    pub source: PathBuf,
    /// Executable recorded by the initializer.
    pub binary: PathBuf,
    /// Git authority shared by the imported source and private worktrees.
    pub git_directory: PathBuf,
}

impl Workspace {
    pub fn open(root: &Path) -> Result<Self> {
        if root.is_symlink() {
            return Err(Issue::InvalidRoot.at(root));
        }
        let root = root.canonicalize().map_err(|error| Error::io(root, error))?;
        let config: Config = records::read(&root.join("workspace.json"))?;
        if config.location != root {
            return Err(Issue::ChangedDirectory.at(&root));
        }
        Ok(Self { root, config })
    }

    pub fn create(request: &CreateRequest) -> Result<Self> {
        let repository = Repository::discover(&request.source)?;
        if paths::exists(&request.root)? {
            return Err(Issue::InvalidRoot.at(&request.root));
        }
        let root = canonical_target(&request.root)?;
        if root.starts_with(&repository.root) {
            return Err(Issue::InvalidRoot.at(&root));
        }
        let parent = root.parent().ok_or_else(|| Issue::InvalidRoot.at(&root))?;
        let policy = submodules::admit(&repository, &request.policy)?;
        if !cowtree::inspect_path(parent)?.supported() {
            return Err(cowtree::Error::UnsupportedPlatform.into());
        }
        let private = tempfile::Builder::new()
            .prefix(".cowtree-init-")
            .tempdir_in(parent)
            .map_err(|error| Error::io(parent, error))?
            .keep();
        let result = Self::publish_initialization(request, &repository, &root, &private, policy);
        if paths::exists(&private)? {
            if let Err(cleanup) = records::remove_directory(&private) {
                return Err(match result {
                    Ok(_) => cleanup,
                    Err(original) => {
                        Error::Cleanup { original: Box::new(original), cleanup: Box::new(cleanup) }
                    }
                });
            }
        }
        result
    }

    fn publish_initialization(
        request: &CreateRequest,
        repository: &Repository,
        root: &Path,
        private: &Path,
        policy: Policy,
    ) -> Result<Self> {
        let lock_path = private.join("lock");
        let lock = File::options()
            .read(true)
            .append(true)
            .create(true)
            .open(&lock_path)
            .map_err(|error| Error::io(&lock_path, error))?;
        fs4::FileExt::lock(&lock).map_err(|error| Error::io(&lock_path, error))?;
        let mut repository = repository.clone();
        repository.locks.push(Arc::new(lock));
        let binary =
            request.program.canonicalize().map_err(|error| Error::io(&request.program, error))?;
        let initialization = Initialization {
            target: root.into(),
            source: repository.root.clone(),
            binary,
            git_directory: repository.directory.clone(),
        };
        records::write(&private.join("initialization.json"), &initialization)?;
        let staging = Self::staging(root)?;
        durability::publish_directory(private, &staging)?;
        crate::fault::checkpoint("after-initialization-intent");
        if paths::exists(root)? {
            records::remove_directory(&staging)?;
            return Err(Issue::InvalidRoot.at(root));
        }
        Self::initialize(&staging, &initialization, repository, policy)?;
        durability::publish_directory(&staging, root)?;
        Self::open(root)
    }

    fn initialize(
        staging: &Path,
        initialization: &Initialization,
        repository: Repository,
        policy: Policy,
    ) -> Result<()> {
        for name in DIRECTORIES {
            fs::create_dir(staging.join(name)).map_err(|error| Error::io(staging, error))?;
        }
        let node = {
            let repository = repository.locked()?;
            let (commit, pins) = repository.snapshot(policy.submodules)?;
            if pins != policy.pins {
                return Err(Issue::HeadChanged.at(&repository.root));
            }
            let nodes = Nodes {
                directory: staging.join("nodes"),
                repository: repository.clone(),
                policy: policy.clone(),
            };
            let node = nodes.seal(
                &repository.root,
                Seal { git_parent: Some(commit.clone()), ..Seal::default() },
            )?;
            if repository.head()? != commit {
                return Err(Issue::HeadChanged.at(&repository.root));
            }
            nodes.publish(&node)?;
            node
        };
        let config = Config {
            location: initialization.target.clone(),
            source: initialization.source.clone(),
            git_directory: initialization.git_directory.clone(),
            binary: initialization.binary.clone(),
            policy,
            initial: node.id.clone(),
            warm_tip: node.id,
        };
        Store::create(&staging.join("authority"), Limits::default())?;
        records::write(&staging.join("import.json"), &config)?;
        imports::run(staging, &config, repository.locks.clone())
    }

    pub(crate) fn staging(root: &Path) -> Result<PathBuf> {
        let name = root
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| Issue::NonUtf8.at(root))?;
        Ok(root.with_file_name(format!(".{name}.cowtree-init")))
    }
}

pub(crate) fn canonical_target(path: &Path) -> Result<PathBuf> {
    let path = std::path::absolute(path).map_err(|error| Error::io(path, error))?;
    let parent = path.parent().ok_or_else(|| Issue::InvalidRoot.at(&path))?;
    let parent = parent.canonicalize().map_err(|error| Error::io(parent, error))?;
    let name = path.file_name().ok_or_else(|| Issue::InvalidRoot.at(&path))?;
    Ok(parent.join(name))
}
