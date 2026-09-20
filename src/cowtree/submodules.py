"""Admit pinned dependencies and verify private materializations without updating their refs.

Submodule contents become read-only source in the managed Git projection.
Their original Git control directories are never copied into snapshots or leaves.
"""

from dataclasses import replace
import errno
import os
from pathlib import Path
import tempfile
import unicodedata

from pydantic import TypeAdapter

from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.git import GitRepository
from cowtree.install import local_path
from cowtree.metadata_types import Entry, ReadOnlyKind
from cowtree.submodule_types import PinnedSubmodule
from cowtree.tree_types import PathPolicy
from cowtree.types import FileMode, TrackedFile


PIN_FILE = ".cowtree-pin"
PIN = TypeAdapter(PinnedSubmodule)


def admit(repository: GitRepository, policy: PathPolicy) -> PathPolicy:
    """Pin clean initialized children before reserving a workspace directory."""
    if policy.pins:
        raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "pins are derived from Git")
    checkout = repository.snapshot(submodules=policy.submodules)
    for pin in checkout.submodules:
        path = local_path(root=repository.path, relative=pin.path)
        if path.is_symlink() or not path.is_dir():
            raise CowtreeError(
                CowtreeErrorCode.SUBMODULE_UNSUPPORTED, f"uninitialized submodule: {pin.path}"
            )
        child = GitRepository.discover(io=repository.io, source=path)
        if child.path != path.resolve():
            raise CowtreeError(
                CowtreeErrorCode.SUBMODULE_UNSUPPORTED, f"uninitialized submodule: {pin.path}"
            )
        snapshot = child.snapshot()
        validate_links(child.path, snapshot.files)
        if snapshot.commit != pin.commit:
            raise CowtreeError(
                CowtreeErrorCode.HEAD_MISMATCH, f"submodule differs from gitlink: {pin.path}"
            )
        verify_tree(child, path, pin.commit)
        if child.capture(args=["ls-files", "--others", "--directory", "-z"]):
            raise CowtreeError(
                CowtreeErrorCode.DIRTY_SOURCE, f"untracked submodule contents: {pin.path}"
            )
        if os.path.lexists(path / PIN_FILE):
            raise CowtreeError(
                CowtreeErrorCode.INVALID_ARGUMENTS, f"reserved dependency marker: {pin.path}"
            )
        for prefix in (*policy.derived, *policy.ephemeral, *policy.ignored):
            if overlaps(pin.path, prefix):
                raise CowtreeError(
                    CowtreeErrorCode.INVALID_ARGUMENTS,
                    f"dependency conflicts with path policy: {pin.path}",
                )
    result = replace(policy, pins=checkout.submodules)
    return result


def overlaps(left: str, right: str) -> bool:
    result = left == right or left.startswith(right + "/") or right.startswith(left + "/")
    return result


def validate_links(root: Path, files: tuple[TrackedFile, ...]) -> None:
    """Copied dependency links cannot retain references to the source or outside files."""
    for entry in files:
        if entry.mode is not FileMode.SYMLINK:
            continue
        path = local_path(root, entry.path)
        link = Path(os.readlink(path))
        logical = Path(os.path.normpath(path.parent / link))
        try:
            resolved = path.resolve()
        except (RuntimeError, OSError) as error:
            if isinstance(error, OSError) and error.errno != errno.ELOOP:
                raise
            raise CowtreeError(
                CowtreeErrorCode.SUBMODULE_UNSUPPORTED, f"cyclic dependency link: {path}"
            ) from error
        if (
            link.is_absolute()
            or not logical.is_relative_to(root)
            or not resolved.is_relative_to(root)
        ):
            raise CowtreeError(
                CowtreeErrorCode.SUBMODULE_UNSUPPORTED, f"dependency link escapes capture: {path}"
            )


def materialize(repository: GitRepository, source: Path, target: Path, policy: PathPolicy) -> None:
    """Verify copied dependency bytes against fresh Git indexes before recording their pins."""
    for pin in policy.pins:
        child = GitRepository.discover(io=repository.io, source=source / pin.path)
        if child.head() != pin.commit:
            raise CowtreeError(
                CowtreeErrorCode.HEAD_MISMATCH, f"submodule HEAD changed: {pin.path}"
            )
        copied = local_path(root=target, relative=pin.path)
        verify_tree(child, copied, pin.commit)
        with (copied / PIN_FILE).open("xb") as stream:
            stream.write(PIN.dump_json(pin))


def verify_tree(repository: GitRepository, tree: Path, commit: str) -> None:
    """Use an owned stat-empty index so caller index caches cannot hide changed bytes."""
    directory = repository.capture(
        args=["rev-parse", "--path-format=absolute", "--git-dir"]
    ).removesuffix("\n")
    command = [
        "git",
        "--git-dir",
        directory,
        "--work-tree",
        str(tree),
        "-c",
        "core.filemode=true",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "core.ignorestat=false",
    ]
    with tempfile.TemporaryDirectory(prefix="cowtree-pin-index-") as scratch:
        environment = {**os.environ, "GIT_INDEX_FILE": str(Path(scratch) / "index")}
        repository.io.run(argv=[*command, "read-tree", commit], env=environment)
        result = repository.io.run(
            argv=[*command, "status", "--porcelain=v1", "-z", "--untracked-files=no"],
            env=environment,
        )
    if result.stdout:
        raise CowtreeError(
            CowtreeErrorCode.DIRTY_SOURCE, f"dependency differs from pinned tree: {tree}"
        )


def scope(path: str, policy: PathPolicy) -> str | None:
    for pin in policy.pins:
        prefix = unicodedata.normalize("NFC", pin.path)
        if path == prefix or path.startswith(prefix + "/"):
            return prefix
    return None


def verify_manifest(actual: dict[str, Entry], expected: dict[str, Entry]) -> None:
    """A checkpoint cannot change, remove, or extend an inherited dependency namespace."""
    for path in actual.keys() | expected.keys():
        before = expected.get(path)
        after = actual.get(path)
        protected = any(
            entry is not None and isinstance(entry.kind, ReadOnlyKind) for entry in (before, after)
        )
        if protected and before != after:
            raise CowtreeError(
                CowtreeErrorCode.DEPENDENCY_CHANGED, f"read-only dependency changed: {path}"
            )
