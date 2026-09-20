"""Capture and clone quiescent trees with explicit cache and ephemeral policy."""

from __future__ import annotations

import hashlib
import os
from pathlib import Path
import shutil
import stat
import unicodedata

from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.native import clone_regular_file
from cowtree.tree_types import DerivedHardlinks, PathClass, PathPolicy, TreeEntry, TreeKind


def validate_policy(policy: PathPolicy) -> None:
    """Reject ambiguous or non-relative policy prefixes before reading files."""
    prefixes = (*policy.derived, *policy.ephemeral)
    for prefix in (*prefixes, *policy.ignored):
        if (
            not prefix
            or "\0" in prefix
            or any(
                part.casefold() in ("", ".", "..", ".git", ".cowtree") for part in prefix.split("/")
            )
        ):
            raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, f"invalid prefix: {prefix!r}")
    for index, left in enumerate(prefixes):
        for right in prefixes[index + 1 :]:
            if overlaps(left=left, right=right):
                raise CowtreeError(
                    CowtreeErrorCode.INVALID_ARGUMENTS, f"overlapping prefixes: {left}, {right}"
                )


def overlaps(left: str, right: str) -> bool:
    """Compare path components rather than textual prefix collisions."""
    result = left == right or left.startswith(right + "/") or right.startswith(left + "/")
    return result


def classify(path: str, policy: PathPolicy) -> PathClass:
    """Keep repository control state out of every captured tree."""
    path = unicodedata.normalize("NFC", path)
    if any(part.casefold() in (".git", ".cowtree") for part in path.split("/")):
        return PathClass.EPHEMERAL
    for prefix in policy.ephemeral:
        prefix = unicodedata.normalize("NFC", prefix)
        if path == prefix or path.startswith(prefix + "/"):
            return PathClass.EPHEMERAL
    for prefix in policy.derived:
        prefix = unicodedata.normalize("NFC", prefix)
        if path == prefix or path.startswith(prefix + "/"):
            return PathClass.DERIVED
    for prefix in policy.ignored:
        prefix = unicodedata.normalize("NFC", prefix)
        if path == prefix or path.startswith(prefix + "/"):
            return PathClass.EPHEMERAL
    return PathClass.SOURCE


def capture_entry(
    root: Path, path: str, classification: PathClass, policy: PathPolicy
) -> TreeEntry:
    """Hash source bytes and capture clone metadata for one supported entry."""
    source = root / path
    before = source.lstat()
    mode = stat.S_IMODE(before.st_mode)
    digest: str | None = None
    link: str | None = None
    if stat.S_ISDIR(before.st_mode):
        kind = TreeKind.DIRECTORY
    elif stat.S_ISLNK(before.st_mode):
        kind = TreeKind.SYMLINK
        link = os.readlink(source)
        digest = hashlib.sha256(os.fsencode(link)).hexdigest()
    elif stat.S_ISREG(before.st_mode):
        if before.st_nlink != 1 and not (
            classification is PathClass.DERIVED
            and policy.derived_hardlinks is DerivedHardlinks.CLONE
        ):
            raise CowtreeError(CowtreeErrorCode.UNSUPPORTED_MODE, f"hard-linked file: {path}")
        kind = TreeKind.FILE
        if classification is PathClass.SOURCE:
            checksum = hashlib.sha256()
            with source.open("rb") as stream:
                for block in iter(lambda: stream.read(1024 * 1024), b""):
                    checksum.update(block)
            digest = checksum.hexdigest()
    else:
        raise CowtreeError(CowtreeErrorCode.UNSUPPORTED_MODE, f"unsupported file type: {path}")
    after = source.lstat()
    identity = ("st_dev", "st_ino", "st_mode", "st_size", "st_mtime_ns", "st_ctime_ns")
    if any(getattr(before, name) != getattr(after, name) for name in identity):
        raise CowtreeError(CowtreeErrorCode.DIRTY_SOURCE, f"source changed during capture: {path}")
    result = TreeEntry(
        path, kind, classification, mode, before.st_size, before.st_mtime_ns, digest, link
    )
    return result


def scan_tree(root: Path, policy: PathPolicy) -> tuple[TreeEntry, ...]:
    """Inventory a quiescent tree without traversing symlinks or ephemeral paths."""
    validate_policy(policy=policy)
    if root.is_symlink() or not root.is_dir():
        raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, f"not a tree directory: {root}")
    pending = [root]
    entries: list[TreeEntry] = []
    while pending:
        directory = pending.pop()
        for source in sorted(directory.iterdir()):
            path = source.relative_to(root).as_posix()
            classification = classify(path=path, policy=policy)
            if classification is PathClass.EPHEMERAL:
                selected_child = any(
                    unicodedata.normalize("NFC", prefix).startswith(
                        unicodedata.normalize("NFC", path) + "/"
                    )
                    for prefix in policy.derived
                )
                if not selected_child or source.is_symlink() or not source.is_dir():
                    continue
                classification = PathClass.DERIVED
            entry = capture_entry(
                root=root, path=path, classification=classification, policy=policy
            )
            entries.append(entry)
            if entry.kind is TreeKind.DIRECTORY:
                pending.append(source)
    result = tuple(sorted(entries, key=lambda entry: entry.path))
    return result


def clone_tree(source: Path, target: Path, policy: PathPolicy) -> tuple[TreeEntry, ...]:
    """Clone source and cache files, retaining only an entirely populated target."""
    source = source.resolve()
    target = target.absolute()
    if os.path.lexists(target) or target.resolve().is_relative_to(source):
        raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, f"invalid clone target: {target}")
    target.mkdir()
    try:
        entries = populate_tree(source=source, target=target, policy=policy)
    except BaseException as original:
        try:
            shutil.rmtree(target)
        except OSError as cleanup:
            raise CowtreeError(
                CowtreeErrorCode.CLEANUP_FAILED,
                f"tree clone failed: {original}; cleanup: {cleanup}",
            ) from original
        raise
    return entries


def populate_tree(source: Path, target: Path, policy: PathPolicy) -> tuple[TreeEntry, ...]:
    """Fill an owned empty directory while preserving its Git control entry."""
    if (
        target.is_symlink()
        or not target.is_dir()
        or target.resolve().is_relative_to(source.resolve())
    ):
        raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "invalid population target")
    if any(path.name != ".git" for path in target.iterdir()):
        raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "population target contains data")
    entries = scan_tree(root=source, policy=policy)
    for entry in entries:
        destination = target / entry.path
        if entry.kind is TreeKind.DIRECTORY:
            destination.mkdir()
        elif entry.kind is TreeKind.SYMLINK:
            assert entry.link is not None
            destination.symlink_to(entry.link)
        else:
            clone_regular_file(source=source / entry.path, target=destination)
            cloned = capture_entry(
                root=target, path=entry.path, classification=entry.classification, policy=policy
            )
            if cloned != entry:
                raise CowtreeError(
                    CowtreeErrorCode.DIRTY_SOURCE, f"clone differs from capture: {entry.path}"
                )
    if scan_tree(root=source, policy=policy) != entries:
        raise CowtreeError(CowtreeErrorCode.DIRTY_SOURCE, "source changed during tree clone")
    for entry in reversed(entries):
        if entry.kind is TreeKind.DIRECTORY:
            destination = target / entry.path
            os.chmod(destination, entry.mode)
            os.utime(destination, ns=(entry.mtime_ns, entry.mtime_ns))
    return entries
