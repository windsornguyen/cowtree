"""Install current origins while preserving private and submitted filesystem edits."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
import shutil
import stat
from typing import Literal
import unicodedata
import uuid

from pydantic import Field, TypeAdapter

from cowtree.durable import sync_directory, write_record
from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.exec import CommandRunner
from cowtree.git import GitRepository
from cowtree.install import Change, Installation, InstallRecord, local_path
from cowtree.leaves import Leaves
from cowtree.metadata import Metadata, MetadataError
from cowtree.metadata_types import Entry, Grant, MetadataReason, Operation, Record, Tip
from cowtree.nodes import source_manifest
from cowtree.path_policy import check_aliases, working_policy
from cowtree.trees import scan_tree
from cowtree.workspace import Workspace
from cowtree.workspace_types import Leaf


class ViewRecord(Record):
    kind: Literal["acquire", "sync", "discard"]
    before: Leaf
    tokens: list[int]
    paths: list[str]
    grants: list[Grant] = Field(default_factory=list)
    after: Leaf | None = None


class PublishedView(Record):
    tip: Tip
    entries: dict[str, Entry]


class Acquisition(Record):
    leaf: Leaf
    changes: list[Change]


@dataclass(frozen=True)
class Views:
    workspace: Workspace

    def current(self, leaf: Leaf) -> dict[str, Entry]:
        identity = leaf.path.lstat()
        if not stat.S_ISDIR(identity.st_mode) or (identity.st_dev, identity.st_ino) != (
            leaf.device,
            leaf.inode,
        ):
            raise CowtreeError(
                CowtreeErrorCode.INVALID_ARGUMENTS, "leaf directory identity changed"
            )
        GitRepository(io=CommandRunner(), path=leaf.path).check_detached(expected=(leaf.git_head,))
        policy = working_policy(source=leaf.path, policy=self.workspace.config.policy)
        result = source_manifest(
            root=leaf.path, entries=scan_tree(root=leaf.path, policy=policy), policy=policy
        )
        return result

    @staticmethod
    def snapshot(metadata: Metadata) -> PublishedView:
        tip = metadata.call(Operation.TIP).decode("tip", TypeAdapter(Tip))
        manifest = metadata.call(Operation.SNAPSHOT, {"version": tip.version}).decode(
            "snapshot", TypeAdapter(dict[str, Entry])
        )
        result = PublishedView(tip=tip, entries=manifest)
        return result

    def acquire(self, identity: int, paths: tuple[str, ...]) -> Leaf:
        with self.workspace.session() as metadata:
            self.recover_locked(metadata=metadata)
            leaf = Leaves(self.workspace).read(identity=identity)
            current = self.current(leaf=leaf)
            published = self.snapshot(metadata=metadata)
            tip = published.entries
            requested = {unicodedata.normalize("NFC", path) for path in paths}
            if not requested:
                raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "acquire requires paths")
            for path in requested:
                local_path(root=leaf.path, relative=path)
            selected = requested | {
                path
                for path in set(current) | set(tip)
                if any(path.startswith(prefix + "/") for prefix in requested)
            }
            live = metadata.call(Operation.GRANTS).decode("grants", TypeAdapter(list[Grant]))
            check_aliases(paths=set(current) | set(tip) | selected | {grant.path for grant in live})
            record = ViewRecord(
                kind="acquire",
                before=leaf,
                tokens=[grant.token for grant in live if grant.leaf == identity],
                paths=sorted(selected),
            )
            directory = self.begin(record=record)
            grants = metadata.call(
                Operation.ACQUIRE, {"leaf": identity, "paths": record.paths}
            ).decode("grants", TypeAdapter(list[Grant]))
            plan = self.acquisition(leaf=leaf, current=current, grants=grants)
            updated = plan.leaf
            record = record.model_copy(update={"grants": grants, "after": updated})
            write_record(path=directory / "view.json", record=record)
            Installation.prepare(
                directory=directory / "installation",
                record=InstallRecord(root=leaf.path, changes=plan.changes),
                objects=self.workspace.root / "authority/objects",
            )
            self.complete(metadata=metadata, directory=directory, record=record)
            return updated

    @staticmethod
    def acquisition(leaf: Leaf, current: dict[str, Entry], grants: list[Grant]) -> Acquisition:
        origins = dict(leaf.origins)
        held = dict(leaf.grants)
        changes: list[Change] = []
        for grant in grants:
            value = current.get(grant.path)
            original = leaf.origins.get(grant.path)
            dirty = value != original
            if dirty and original != grant.origin and value != grant.origin:
                raise CowtreeError(
                    CowtreeErrorCode.DIRTY_SOURCE, f"source needs explicit resolution: {grant.path}"
                )
            desired = value if dirty and original == grant.origin else grant.origin
            if desired != value or grant.origin != original:
                changes.append(Change(path=grant.path, before=value, after=desired))
            if grant.origin is None:
                origins.pop(grant.path, None)
            else:
                origins[grant.path] = grant.origin
            held[grant.path] = grant.model_copy(update={"activated": True})
        result = Acquisition(
            leaf=leaf.model_copy(update={"origins": origins, "grants": held}), changes=changes
        )
        return result

    def sync(self, identity: int) -> Leaf:
        with self.workspace.session() as metadata:
            self.recover_locked(metadata=metadata)
            leaf = Leaves(self.workspace).read(identity=identity)
            current = self.current(leaf=leaf)
            published = self.snapshot(metadata=metadata)
            tip = published.entries
            blocked = {
                path
                for path in set(current) | set(leaf.origins)
                if current.get(path) != leaf.origins.get(path)
            }
            if leaf.pending is not None:
                blocked.update(leaf.pending.changes)
            selected = (set(current) | set(leaf.origins) | set(tip)) - blocked
            check_aliases(paths=set(current) | set(tip))
            origins = dict(leaf.origins)
            changes: list[Change] = []
            for path in sorted(selected):
                value = tip.get(path)
                if value is None:
                    origins.pop(path, None)
                else:
                    origins[path] = value
                if current.get(path) != value:
                    changes.append(Change(path=path, before=current.get(path), after=value))
            workspace = Workspace.open(root=self.workspace.root)
            warm = workspace.nodes.read(identity=workspace.config.warm_tip)
            if warm.source != tip:
                raise CowtreeError(
                    CowtreeErrorCode.COMMAND_FAILED, "published tip needs projection recovery"
                )
            updated = leaf.model_copy(update={"origins": origins, "git_head": warm.git_commit})
            record = ViewRecord(
                kind="sync", before=leaf, tokens=[], paths=sorted(selected), after=updated
            )
            Installation.preflight(record=InstallRecord(root=leaf.path, changes=changes))
            directory = self.begin(record=record)
            Installation.prepare(
                directory=directory / "installation",
                record=InstallRecord(root=leaf.path, changes=changes),
                objects=self.workspace.root / "authority/objects",
            )
            self.complete(metadata=metadata, directory=directory, record=record)
            return updated

    def discard(self, identity: int, paths: tuple[str, ...]) -> Leaf:
        """Restore selected paths to their submitted value or last installed origin."""
        with self.workspace.session() as metadata:
            self.recover_locked(metadata=metadata)
            leaf = Leaves(self.workspace).read(identity=identity)
            current = self.current(leaf=leaf)
            selected = sorted({unicodedata.normalize("NFC", path) for path in paths})
            if not selected:
                raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "discard requires paths")
            baseline = dict(leaf.origins)
            if leaf.pending is not None:
                for path, value in leaf.pending.changes.items():
                    if value is None:
                        baseline.pop(path, None)
                    else:
                        baseline[path] = value
            for path in selected:
                local_path(root=leaf.path, relative=path)
            changes = [
                Change(path=path, before=current.get(path), after=baseline.get(path))
                for path in selected
                if current.get(path) != baseline.get(path)
            ]
            record = ViewRecord(kind="discard", before=leaf, tokens=[], paths=selected, after=leaf)
            Installation.preflight(record=InstallRecord(root=leaf.path, changes=changes))
            directory = self.begin(record=record)
            Installation.prepare(
                directory=directory / "installation",
                record=InstallRecord(root=leaf.path, changes=changes),
                objects=self.workspace.root / "authority/objects",
            )
            self.complete(metadata=metadata, directory=directory, record=record)
            return leaf

    def begin(self, record: ViewRecord) -> Path:
        directory = self.workspace.root / "operations" / f"view-{uuid.uuid4().hex}"
        directory.mkdir(mode=0o700)
        write_record(path=directory / "view.json", record=record)
        return directory

    def complete(self, metadata: Metadata, directory: Path, record: ViewRecord) -> None:
        assert record.after is not None
        if (
            record.kind == "discard"
            or Leaves(self.workspace).read(identity=record.before.id) != record.after
        ):
            Installation.open(directory=directory / "installation").apply()
            for grant in record.grants:
                metadata.call(Operation.ACTIVATE, {"grant": grant.model_dump(mode="json")}).decode(
                    "grant", TypeAdapter(Grant)
                )
            if record.after.git_head != record.before.git_head:
                target = GitRepository(
                    io=CommandRunner(descriptors=metadata.descriptors), path=record.before.path
                )
                target.check_detached(expected=(record.before.git_head, record.after.git_head))
                target.run(
                    args=["-c", "core.fsync=all", "reset", "--mixed", "-q", record.after.git_head]
                )
            Leaves(self.workspace).save(leaf=record.after)
        shutil.rmtree(directory)
        sync_directory(path=directory.parent)

    def abort(self, metadata: Metadata, directory: Path, record: ViewRecord) -> None:
        if (directory / "installation/install.json").exists():
            Installation.open(directory=directory / "installation").apply(rollback=True)
        if record.kind == "acquire":
            live = metadata.call(Operation.GRANTS).decode("grants", TypeAdapter(list[Grant]))
            for grant in live:
                if (
                    grant.leaf == record.before.id
                    and grant.token not in record.tokens
                    and grant.path in record.paths
                ):
                    metadata.call(Operation.RELEASE, {"grant": grant.model_dump(mode="json")})
        shutil.rmtree(directory)
        sync_directory(path=directory.parent)

    def recover_locked(self, metadata: Metadata) -> None:
        for directory in sorted((self.workspace.root / "operations").glob("view-*")):
            if not (directory / "view.json").exists():
                shutil.rmtree(directory)
                continue
            record = ViewRecord.model_validate_json((directory / "view.json").read_bytes())
            if record.after is None or not (directory / "installation/install.json").exists():
                self.abort(metadata=metadata, directory=directory, record=record)
                continue
            try:
                self.complete(metadata=metadata, directory=directory, record=record)
            except MetadataError as error:
                if error.reason not in (MetadataReason.STALE_TOKEN, MetadataReason.LEAF_INACTIVE):
                    raise
                self.abort(metadata=metadata, directory=directory, record=record)
