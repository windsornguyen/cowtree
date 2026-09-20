"""Path classes and captured filesystem entries for local snapshots."""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum


class PathClass(str, Enum):
    SOURCE = "source"
    DERIVED = "derived"
    EPHEMERAL = "ephemeral"


class TreeKind(str, Enum):
    FILE = "file"
    DIRECTORY = "directory"
    SYMLINK = "symlink"


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
