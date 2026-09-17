"""Retire managed leaves without confusing a reused pathname with owned state."""

from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import dataclass
import fcntl
import os
from pathlib import Path
import shutil
import stat
import uuid

from pydantic import TypeAdapter

from cowtree.durable import sync_directory, write_record
from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.leaves import Leaves
from cowtree.metadata import Metadata
from cowtree.metadata_types import Operation, Record
from cowtree.views import Views
from cowtree.workspace import Workspace
from cowtree.workspace_types import Leaf


class DropRecord(Record):
    leaf: Leaf


class ValidationBusyError(CowtreeError):
    """A surviving validation process still owns its working directory."""

    def __init__(self) -> None:
        super().__init__(CowtreeErrorCode.INVALID_ARGUMENTS, "validation is still running")


@dataclass(frozen=True)
class Lifecycle:
    workspace: Workspace

    def drop(self, identity: int, *, force: bool = False, missing_ok: bool = False) -> None:
        with self.workspace.session() as metadata:
            if (
                missing_ok
                and not (self.workspace.root / "leaves" / f"{identity}.json").exists()
                and identity
                not in metadata.call(Operation.LEAVES).decode("leaves", TypeAdapter(list[int]))
            ):
                return
            leaf = Leaves(self.workspace).read(identity=identity)
            if leaf.pending is not None:
                raise CowtreeError(
                    CowtreeErrorCode.INVALID_ARGUMENTS,
                    "resume or abort the pending publication before dropping",
                )
            if not force and Views(self.workspace).current(leaf=leaf) != leaf.origins:
                raise CowtreeError(CowtreeErrorCode.DIRTY_SOURCE, "leaf has private source changes")
            with self.guard(leaf=leaf) as descriptors:
                directory = self.workspace.root / "operations" / f"drop-{uuid.uuid4().hex}"
                directory.mkdir(mode=0o700)
                record = DropRecord(leaf=leaf)
                write_record(path=directory / "drop.json", record=record)
                self.remove(
                    metadata=metadata, directory=directory, record=record, descriptors=descriptors
                )

    def complete(self, metadata: Metadata, directory: Path, record: DropRecord) -> None:
        with self.guard(leaf=record.leaf) as descriptors:
            self.remove(
                metadata=metadata, directory=directory, record=record, descriptors=descriptors
            )

    @contextmanager
    def guard(self, leaf: Leaf) -> Iterator[tuple[int, ...]]:
        candidate = leaf.check_candidate
        if candidate is None:
            yield ()
            return
        path = (
            self.workspace.root
            / "checks"
            / f"{candidate.request.leaf}-{candidate.request.sequence}.lock"
        )
        with path.open("a+b") as lock:
            try:
                fcntl.flock(lock.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError as error:
                raise ValidationBusyError() from error
            yield (lock.fileno(),)

    def remove(
        self, metadata: Metadata, directory: Path, record: DropRecord, descriptors: tuple[int, ...]
    ) -> None:
        leaf = record.leaf
        if os.path.lexists(leaf.path):
            identity = leaf.path.lstat()
            if not stat.S_ISDIR(identity.st_mode) or (identity.st_dev, identity.st_ino) != (
                leaf.device,
                leaf.inode,
            ):
                raise CowtreeError(
                    CowtreeErrorCode.INVALID_ARGUMENTS, "drop target identity changed"
                )
        active = metadata.call(Operation.LEAVES).decode("leaves", TypeAdapter(list[int]))
        if leaf.id in active:
            metadata.call(Operation.DROP_LEAF, {"leaf": leaf.id})
        repository = Leaves(self.workspace).repository(
            descriptors=(*metadata.descriptors, *descriptors)
        )
        with repository.lock() as descriptor:
            repository = Leaves(self.workspace).repository(
                descriptors=(*metadata.descriptors, *descriptors, descriptor)
            )
            if repository.find_worktree(path=leaf.path) is not None:
                repository.run(
                    args=["worktree", "remove", "--force", "--force", "--", str(leaf.path)]
                )
            elif leaf.path.exists():
                shutil.rmtree(leaf.path)
            sync_directory(path=leaf.path.parent)
        (self.workspace.root / "leaves" / f"{leaf.id}.json").unlink(missing_ok=True)
        sync_directory(path=self.workspace.root / "leaves")
        shutil.rmtree(directory)
        sync_directory(path=directory.parent)

    def recover_locked(self, metadata: Metadata) -> None:
        for directory in sorted((self.workspace.root / "operations").glob("drop-*")):
            path = directory / "drop.json"
            if path.exists():
                record = DropRecord.model_validate_json(path.read_bytes())
                self.complete(metadata=metadata, directory=directory, record=record)
            else:
                shutil.rmtree(directory)
