"""Path classes and captured filesystem entries for local snapshots."""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum

from cowtree.submodule_types import PinnedSubmodule, SubmodulePolicy


class PathClass(str, Enum):
    SOURCE = "source"
    DERIVED = "derived"
    EPHEMERAL = "ephemeral"


class TreeKind(str, Enum):
    FILE = "file"
    DIRECTORY = "directory"
    SYMLINK = "symlink"


class CaptureMode(str, Enum):
    """Capture source hashes or metadata awaiting a pinned manifest check."""

    CONTENT = "content"
    METADATA = "metadata"


@dataclass(frozen=True)
class FileIdentity:
    device: int
    inode: int
    changed_ns: int


class DerivedHardlinks(str, Enum):
    """Reject aliases or explicitly materialize each cache pathname independently."""

    REJECT = "reject"
    CLONE = "clone"


@dataclass(frozen=True)
class PathPolicy:
    """Classify explicit path prefixes; all other paths are source."""

    derived: tuple[str, ...] = ()
    ephemeral: tuple[str, ...] = ()
    ignored: tuple[str, ...] = ()
    derived_hardlinks: DerivedHardlinks = DerivedHardlinks.REJECT
    submodules: SubmodulePolicy = SubmodulePolicy.REJECT
    pins: tuple[PinnedSubmodule, ...] = ()


@dataclass(frozen=True)
class TreeEntry:
    """Capture identity and metadata without following a symbolic link."""

    path: str
    kind: TreeKind
    classification: PathClass
    mode: int
    size: int
    mtime_ns: int
    digest: str | None
    link: str | None = None
    # Compare identities across source scans; cloned inodes are intentionally different.
    identity: FileIdentity | None = field(default=None, compare=False, repr=False)
