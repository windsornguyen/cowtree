"""Immutable private snapshot images with source-only Git projection."""

from __future__ import annotations

from dataclasses import dataclass
import hashlib
from pathlib import Path
import shutil
import unicodedata
import uuid

from cowtree.durable import sync_directory, sync_file, write_record
from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.metadata_types import Entry, EntryKind
from cowtree.path_policy import check_aliases, working_policy
from cowtree.projection import GitProjection, ProjectionSpec
from cowtree.tree_types import PathClass, PathPolicy, TreeEntry, TreeKind
from cowtree.trees import clone_tree, scan_tree
from cowtree.workspace_types import Node


def source_manifest(root: Path, entries: tuple[TreeEntry, ...]) -> dict[str, Entry]:
    """Select source entries and verify their portable UTF-8 spelling."""
    result: dict[str, Entry] = {}
    for entry in entries:
        if entry.classification is not PathClass.SOURCE or entry.kind is TreeKind.DIRECTORY:
            continue
        try:
            entry.path.encode("utf-8")
            if entry.link is not None:
                entry.link.encode("utf-8")
        except UnicodeError as error:
            raise CowtreeError(
                CowtreeErrorCode.INVALID_ARGUMENTS, "workspace paths and links require UTF-8"
            ) from error
        name = unicodedata.normalize("NFC", entry.path)
        if name != entry.path:
            original = (root / entry.path).lstat()
            try:
                canonical = (root / name).lstat()
            except FileNotFoundError as error:
                raise CowtreeError(
                    CowtreeErrorCode.INVALID_ARGUMENTS, f"path needs NFC spelling: {entry.path!r}"
                ) from error
            if (original.st_dev, original.st_ino) != (canonical.st_dev, canonical.st_ino):
                raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "ambiguous Unicode path")
        assert entry.digest is not None
        if entry.kind is TreeKind.SYMLINK:
            kind = EntryKind.SYMLINK
        else:
            kind = EntryKind.EXECUTABLE if entry.mode & 0o100 else EntryKind.FILE
        result[name] = Entry(object=entry.digest, kind=kind)
    check_aliases(paths=set(result))
    return result


@dataclass(frozen=True)
class Nodes:
    """Own snapshot images under one workspace directory."""

    directory: Path
    projection: GitProjection
    policy: PathPolicy

    def read(self, identity: str) -> Node:
        if len(identity) != 64 or any(
            character not in "0123456789abcdef" for character in identity
        ):
            raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "invalid node identity")
        node = Node.model_validate_json((self.directory / identity / "node.json").read_bytes())
        if node.id != identity:
            raise CowtreeError(
                CowtreeErrorCode.COMMAND_FAILED, "node identity does not match its record"
            )
        return node

    def seal(
        self,
        source: Path,
        origins: dict[str, Entry] | None = None,
        parent: str | None = None,
        projection: ProjectionSpec | None = None,
        identity: str | None = None,
    ) -> Node:
        """Capture eligible bytes before making the node visible by record and ref."""
        if projection is None:
            projection = ProjectionSpec()
        if len(list(self.directory.iterdir())) >= 256:
            raise CowtreeError(
                CowtreeErrorCode.INVALID_ARGUMENTS, "snapshot budget exhausted; run collection"
            )
        if identity is None:
            identity = hashlib.sha256(uuid.uuid4().bytes).hexdigest()
        if len(identity) != 64 or any(
            character not in "0123456789abcdef" for character in identity
        ):
            raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "invalid node identity")
        directory = self.directory / identity
        directory.mkdir()
        tree = directory / "tree"
        try:
            policy = working_policy(source=source, policy=self.policy)
            entries = clone_tree(source=source, target=tree, policy=policy)
            check_aliases(
                paths={unicodedata.normalize("NFC", entry.path) for entry in entries}
                | {unicodedata.normalize("NFC", path) for path in self.policy.derived}
                | {unicodedata.normalize("NFC", path) for path in self.policy.ephemeral}
            )
            manifest = source_manifest(root=tree, entries=entries)
            parent_node = None if parent is None else self.read(identity=parent)
            parents = () if projection.parent is None else (projection.parent,)
            if parent_node is not None:
                parents = (parent_node.git_commit,)
            commit = self.projection.create(
                tree=tree, paths=tuple(manifest), parents=parents, reuse=projection.reuse
            )
            node = Node(
                id=identity,
                parent=parent,
                source=manifest,
                origins=manifest if origins is None else origins,
                git_commit=commit,
                policy=policy,
            )
            for entry in entries:
                if entry.kind is TreeKind.FILE:
                    sync_file(path=tree / entry.path)
            for entry in reversed(entries):
                if entry.kind is TreeKind.DIRECTORY:
                    sync_directory(path=tree / entry.path)
            sync_directory(path=tree)
            write_record(path=directory / "node.json", record=node)
            sync_directory(path=self.directory)
        except BaseException:
            shutil.rmtree(directory)
            raise
        self.projection.set_ref(reference=f"refs/cowtree/nodes/{identity}", commit=node.git_commit)
        return node

    def verify(self, node: Node) -> None:
        """Reject modified source bytes before using an owned immutable image."""
        tree = self.directory / node.id / "tree"
        actual = source_manifest(root=tree, entries=scan_tree(root=tree, policy=node.policy))
        if actual != node.source:
            raise CowtreeError(
                CowtreeErrorCode.COMMAND_FAILED, f"snapshot source changed: {node.id}"
            )

    def complete_refs(self) -> None:
        """Finish durable nodes whose Git reference update was interrupted."""
        for directory in self.directory.iterdir():
            if not (directory / "node.json").exists():
                continue  # Incomplete captures are collected separately by their owner.
            node = self.read(identity=directory.name)
            self.projection.set_ref(
                reference=f"refs/cowtree/nodes/{node.id}", commit=node.git_commit
            )
