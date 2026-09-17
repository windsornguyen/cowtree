"""Replayable per-file installation with retained before and after images."""

from __future__ import annotations

from dataclasses import dataclass
import errno
import hashlib
import os
from pathlib import Path
import shutil
import stat

from cowtree.durable import sync_directory, sync_file, write_record
from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.metadata_types import Entry, EntryKind, Record
from cowtree.native import clone_regular_file


class Change(Record):
    path: str
    before: Entry | None
    after: Entry | None


class InstallRecord(Record):
    root: Path
    changes: list[Change]
    device: int = 0
    inode: int = 0


def local_path(root: Path, relative: str) -> Path:
    """Reject parent traversal and symlink parents before touching an entry."""
    parts = relative.split("/")
    if (
        any(part.casefold() in ("", ".", "..", ".git", ".cowtree") for part in parts)
        or "\0" in relative
    ):
        raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, f"invalid source path: {relative!r}")
    target = root / relative
    parent = target.parent
    while parent != root:
        if parent.is_symlink():
            raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, f"symlink parent: {parent}")
        parent = parent.parent
    return target


def fingerprint(path: Path) -> Entry | None:
    """Identify source bytes and Git mode without following the final symlink."""
    try:
        metadata = path.lstat()
    except (FileNotFoundError, NotADirectoryError):
        return None
    if stat.S_ISDIR(metadata.st_mode):
        return None  # Published source manifests contain files, not empty directories.
    if stat.S_ISLNK(metadata.st_mode):
        data = os.fsencode(os.readlink(path))
        result = Entry(object=hashlib.sha256(data).hexdigest(), kind=EntryKind.SYMLINK)
        return result
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_nlink != 1:
        raise CowtreeError(CowtreeErrorCode.UNSUPPORTED_MODE, f"unsupported source entry: {path}")
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for data in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(data)
    kind = EntryKind.EXECUTABLE if metadata.st_mode & stat.S_IXUSR else EntryKind.FILE
    result = Entry(object=digest.hexdigest(), kind=kind)
    return result


def save_image(source: Path, target: Path, entry: Entry) -> None:
    """Create a durable owned image of one existing filesystem entry."""
    if entry.kind is EntryKind.SYMLINK:
        target.symlink_to(os.readlink(source))
    else:
        clone_regular_file(source=source, target=target)
        sync_file(path=target)
    if fingerprint(path=target) != entry:
        raise CowtreeError(CowtreeErrorCode.DIRTY_SOURCE, f"entry changed during capture: {source}")


def stage_object(source: Path, target: Path, entry: Entry) -> None:
    """Materialize verified object bytes into their declared source file kind."""
    if entry.kind is EntryKind.SYMLINK:
        data = source.read_bytes()
        if hashlib.sha256(data).hexdigest() != entry.object:
            raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, f"corrupt object: {entry.object}")
        target.symlink_to(data.decode("utf-8"))
    else:
        clone_regular_file(source=source, target=target)
        target.chmod(0o755 if entry.kind is EntryKind.EXECUTABLE else 0o644)
        os.utime(target, None)
        sync_file(path=target)
    if fingerprint(path=target) != entry:
        raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, f"corrupt object: {entry.object}")


def prune_empty_parents(path: Path, root: Path) -> None:
    """Remove only empty ancestors created or emptied within the managed root."""
    while path != root:
        try:
            path.rmdir()
        except OSError as error:
            if error.errno in (errno.ENOTEMPTY, errno.EEXIST):
                return
            raise
        sync_directory(path=path.parent)
        path = path.parent


def create_parents(path: Path, root: Path) -> None:
    """Persist each newly created parent before installing its child's name."""
    parent = root
    for part in path.relative_to(root).parts:
        directory = parent / part
        if not directory.exists():
            directory.mkdir()
            sync_directory(path=parent)
        parent = directory


def remove_empty_tree(path: Path) -> None:
    """Remove directory structure only when no file or symlink occupies it."""
    if not path.is_dir() or path.is_symlink():
        return
    for directory, _, _ in os.walk(path, topdown=False, followlinks=False):
        Path(directory).rmdir()
    sync_directory(path=path.parent)


@dataclass(frozen=True)
class Installation:
    """Own a durable change set until its caller reconciles metadata and finishes."""

    directory: Path
    record: InstallRecord

    @staticmethod
    def preflight(record: InstallRecord) -> None:
        """Reject an impossible final namespace before recording installation intent."""
        root = record.root
        if not root.is_absolute() or root.is_symlink() or not root.is_dir():
            raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "installation root is invalid")
        changes = {change.path: change for change in record.changes}
        if len(changes) != len(record.changes):
            raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "duplicate installation path")
        deleted = {
            change.path
            for change in record.changes
            if change.before is not None and change.after is None
        }
        for change in record.changes:
            target = local_path(root=root, relative=change.path)
            if fingerprint(path=target) != change.before:
                raise CowtreeError(CowtreeErrorCode.DIRTY_SOURCE, f"local edit: {change.path}")
            if change.after is None:
                continue
            parent = target.parent
            while parent != root:
                relative = parent.relative_to(root).as_posix()
                planned = changes.get(relative)
                if (planned is not None and planned.after is not None) or (
                    planned is None and parent.exists() and not parent.is_dir()
                ):
                    raise CowtreeError(
                        CowtreeErrorCode.DIRTY_SOURCE, f"namespace conflict: {relative}"
                    )
                parent = parent.parent
            if not target.is_dir() or target.is_symlink():
                continue
            for directory, directories, files in os.walk(target, followlinks=False):
                prefix = Path(directory).relative_to(root).as_posix() + "/"
                if not any(path.startswith(prefix) for path in deleted):
                    raise CowtreeError(
                        CowtreeErrorCode.DIRTY_SOURCE, f"namespace conflict: {prefix}"
                    )
                for name in [*directories, *files]:
                    child = Path(directory) / name
                    if child.is_dir() and not child.is_symlink():
                        continue
                    if child.relative_to(root).as_posix() not in deleted:
                        raise CowtreeError(
                            CowtreeErrorCode.DIRTY_SOURCE, f"namespace conflict: {child}"
                        )

    @classmethod
    def prepare(cls, directory: Path, record: InstallRecord, objects: Path) -> Installation:
        cls.preflight(record=record)
        root = record.root
        metadata = root.stat()
        if metadata.st_dev != directory.parent.stat().st_dev:
            raise CowtreeError(
                CowtreeErrorCode.DIFFERENT_FILESYSTEM, "journal and leaf must share a filesystem"
            )
        record = record.model_copy(update={"device": metadata.st_dev, "inode": metadata.st_ino})
        directory.mkdir()
        try:
            for index, change in enumerate(record.changes):
                target = local_path(root=root, relative=change.path)
                if fingerprint(path=target) != change.before:
                    raise CowtreeError(CowtreeErrorCode.DIRTY_SOURCE, f"local edit: {change.path}")
                if change.before == change.after:
                    continue
                if change.before is not None:
                    save_image(
                        source=target, target=directory / f"before-{index}", entry=change.before
                    )
                if change.after is not None:
                    stage_object(
                        source=objects / change.after.object,
                        target=directory / f"after-{index}",
                        entry=change.after,
                    )
            sync_directory(path=directory)
            write_record(path=directory / "install.json", record=record)
            sync_directory(path=directory.parent)
        except BaseException:
            shutil.rmtree(directory)
            raise
        result = cls(directory=directory, record=record)
        return result

    @classmethod
    def open(cls, directory: Path) -> Installation:
        record = InstallRecord.model_validate_json((directory / "install.json").read_bytes())
        result = cls(directory=directory, record=record)
        return result

    def apply(self, *, rollback: bool = False) -> None:
        """Replay without overwriting a file whose bytes changed after preparation."""
        root = self.record.root
        metadata = root.lstat()
        if not stat.S_ISDIR(metadata.st_mode) or (metadata.st_dev, metadata.st_ino) != (
            self.record.device,
            self.record.inode,
        ):
            raise CowtreeError(
                CowtreeErrorCode.INVALID_ARGUMENTS, "installation root identity changed"
            )
        operations = list(enumerate(self.record.changes))
        operations.sort(
            key=lambda item: (
                (item[1].before if rollback else item[1].after) is not None,
                -item[1].path.count("/")
                if (item[1].before if rollback else item[1].after) is None
                else item[1].path.count("/"),
            )
        )
        for index, change in operations:
            expected = change.after if rollback else change.before
            desired = change.before if rollback else change.after
            target = local_path(root=self.record.root, relative=change.path)
            current = fingerprint(path=target)
            if current == desired:
                if desired is not None and desired.kind is not EntryKind.SYMLINK:
                    sync_file(path=target)
                parent = target.parent
                while not parent.is_dir() and parent != root:
                    parent = parent.parent
                sync_directory(path=parent)
                continue
            if current != expected:
                raise CowtreeError(
                    CowtreeErrorCode.DIRTY_SOURCE, f"installation conflict: {change.path}"
                )
            if desired is None:
                target.unlink()
                sync_directory(path=target.parent)
                prune_empty_parents(path=target.parent, root=self.record.root)
                continue
            create_parents(path=target.parent, root=root)
            remove_empty_tree(path=target)
            image = self.directory / f"{'before' if rollback else 'after'}-{index}"
            temporary = self.directory / f"{'rollback' if rollback else 'apply'}-{index}"
            if not os.path.lexists(temporary):
                save_image(source=image, target=temporary, entry=desired)
                sync_directory(path=self.directory)
            if fingerprint(path=temporary) != desired:
                raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, "installation image changed")
            os.replace(temporary, target)
            sync_directory(path=target.parent)
            sync_directory(path=self.directory)

    def finish(self) -> None:
        """Remove retained images only after the caller's metadata is durable."""
        shutil.rmtree(self.directory)
        sync_directory(path=self.directory.parent)
