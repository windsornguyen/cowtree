// Copyright (c) 2026 Windsor Nguyen

//! Build source-only Git commits with private indexes and compare-and-swap refs.

use std::{path::Path, process::Command};

use cowtree_metadata::Snapshot;

use crate::{CommitId, Error, Result, error::Issue, git::Repository, paths};

impl Repository {
    pub(crate) fn project(
        &self,
        tree: &Path,
        manifest: &Snapshot,
        parents: &[CommitId],
        reuse: Option<&CommitId>,
    ) -> Result<CommitId> {
        let directory = tempfile::Builder::new()
            .prefix(".cowtree-index-")
            .tempdir_in(tree.parent().ok_or_else(|| Issue::InvalidRoot.at(tree))?)
            .map_err(|error| Error::io(tree, error))?;
        let common = &self.directory;
        let command = || -> Result<Command> {
            let mut command = self.command()?;
            command
                .args(["--literal-pathspecs", "--git-dir"])
                .arg(common)
                .arg("--work-tree")
                .arg(tree)
                .args([
                    "-c",
                    "core.filemode=true",
                    "-c",
                    "core.fsmonitor=false",
                    "-c",
                    "core.sparseCheckout=false",
                ])
                .env("GIT_INDEX_FILE", directory.path().join("index"));
            Ok(command)
        };
        if !manifest.is_empty() {
            let mut input = Vec::new();
            for path in manifest.keys() {
                paths::local(tree, path.as_str())?;
                input.extend_from_slice(path.as_str().as_bytes());
                input.push(0);
            }
            self.input(
                command()?.args([
                    "add",
                    "--force",
                    "--pathspec-from-file=-",
                    "--pathspec-file-nul",
                ]),
                &input,
            )?;
        }
        let tree_id = self.text(command()?.arg("write-tree"))?;
        if let Some(commit) = reuse {
            self.verify_projection(tree, commit, &tree_id, parents)?;
            return Ok(commit.clone());
        }
        let mut commit = command()?;
        commit.args(["commit-tree", &tree_id]);
        for parent in parents {
            commit.args(["-p", parent.as_str()]);
        }
        CommitId::parse(self.text(commit.args(["-m", "cowtree snapshot"]))?)
    }

    fn verify_projection(
        &self,
        tree: &Path,
        commit: &CommitId,
        tree_id: &str,
        parents: &[CommitId],
    ) -> Result<()> {
        let record = self.text(self.command()?.args([
            "show",
            "--no-patch",
            "--no-show-signature",
            "--format=%T%x00%P",
            commit.as_str(),
            "--",
        ]))?;
        let (identity, ancestry) =
            record.split_once('\0').ok_or_else(|| Issue::IncompleteRecord.at(tree))?;
        let expected: Vec<_> = parents.iter().map(CommitId::as_str).collect();
        if identity != tree_id || ancestry.split_whitespace().collect::<Vec<_>>() != expected {
            return Err(Issue::NodeChanged.at(tree));
        }
        Ok(())
    }

    pub(crate) fn set_ref(
        &self,
        reference: &str,
        commit: &CommitId,
        expected: Option<&CommitId>,
    ) -> Result<()> {
        self.check_ref(reference)?;
        let current = self.output(self.command()?.args([
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            reference,
        ]))?;
        if current.status.success() && current.stdout == format!("{}\n", commit.as_str()).as_bytes()
        {
            return Ok(());
        }
        if !matches!(current.status.code(), Some(0 | 1)) {
            return Err(cowtree_git::Error::Command {
                status: current.status,
                stderr: String::from_utf8_lossy(&current.stderr).into_owned(),
            }
            .into());
        }
        let absent = "0".repeat(commit.as_str().len());
        let previous = expected.map_or(absent.as_str(), CommitId::as_str);
        self.capture(self.command()?.args(["update-ref", reference, commit.as_str(), previous]))?;
        Ok(())
    }

    pub(crate) fn remove_ref(&self, reference: &str, expected: &CommitId) -> Result<()> {
        self.check_ref(reference)?;
        let current = self.output(self.command()?.args([
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            reference,
        ]))?;
        if current.status.code() == Some(1) {
            return Ok(());
        }
        if !current.status.success() {
            return Err(cowtree_git::Error::Command {
                status: current.status,
                stderr: String::from_utf8_lossy(&current.stderr).into_owned(),
            }
            .into());
        }
        self.capture(self.command()?.args(["update-ref", "-d", reference, expected.as_str()]))?;
        Ok(())
    }

    fn check_ref(&self, reference: &str) -> Result<()> {
        if !reference.starts_with("refs/cowtree/") {
            return Err(Issue::OutsideNamespace.at(Path::new(reference)));
        }
        self.capture(self.command()?.args(["check-ref-format", reference]))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cowtree_metadata::{Entry, EntryKind, ResourcePath, objects::ObjectId};

    #[test]
    fn projections_isolate_the_index_and_verify_reused_tree_and_ancestry()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        let repository = cowtree_git::Client::at(root);
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.name", "Test"],
            vec!["config", "user.email", "test@example.invalid"],
        ] {
            repository.capture(repository.command()?.args(args))?;
        }
        let repository = Repository::discover(root)?;
        std::fs::write(root.join("selected file"), b"selected")?;
        std::fs::write(root.join("unselected"), b"private")?;
        repository.capture(repository.command()?.args(["add", "unselected"]))?;
        let before = repository.capture(repository.command()?.args(["ls-files", "--stage"]))?;
        let selected = Snapshot::from([(
            ResourcePath::parse("selected file")?,
            Entry { object: ObjectId::from_bytes(b"selected"), kind: EntryKind::File },
        )]);
        let initial = repository.project(root, &selected, &[], None)?;
        assert_eq!(
            repository.text(repository.command()?.args([
                "ls-tree",
                "--name-only",
                initial.as_str()
            ]))?,
            "selected file"
        );
        let empty = repository.project(root, &Snapshot::new(), &[], None)?;
        assert!(
            repository
                .capture(repository.command()?.args(["ls-tree", "--name-only", empty.as_str()]))?
                .is_empty()
        );
        let parents = [initial.clone(), empty.clone()];
        let merged = repository.project(root, &selected, &parents, None)?;
        assert_eq!(repository.project(root, &selected, &parents, Some(&merged))?, merged);
        assert!(repository.project(root, &selected, &[empty, initial], Some(&merged)).is_err());
        assert!(repository.project(root, &Snapshot::new(), &parents, Some(&merged)).is_err());
        assert_eq!(
            repository.capture(repository.command()?.args(["ls-files", "--stage"]))?,
            before
        );
        Ok(())
    }
}
