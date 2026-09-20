"""Durable filesystem state surrounding the SQLite publication authority."""

from __future__ import annotations

from pathlib import Path

from pydantic import Field, PositiveInt

from cowtree.metadata_types import (
    BatchCandidate,
    Candidate,
    Digest,
    Entry,
    Grant,
    Receipt,
    Record,
    Request,
)
from cowtree.tree_types import PathPolicy


class Node(Record):
    id: Digest
    parent: Digest | None
    source: dict[str, Entry]
    origins: dict[str, Entry]
    git_commit: str
    policy: PathPolicy


class Validation(Record):
    candidate: Candidate
    command: tuple[str, ...]
    log: str
    node: Digest


class PublicationRecord(Record):
    receipt: Receipt
    validation: Validation
    receipts: tuple[Receipt, ...] = ()


class Publication(Record):
    request: Request
    node: Digest
    changes: dict[str, Entry | None]
    submitted: bool = False
    aborting: bool = False
    candidate: Candidate | None = None
    batch: BatchCandidate | None = None
    parent_node: Digest | None = None
    validation: Validation | None = None


class Leaf(Record):
    id: PositiveInt
    path: Path
    device: int
    inode: int
    node: Digest
    git_head: str
    origins: dict[str, Entry]
    grants: dict[str, Grant] = Field(default_factory=dict)
    sequence: PositiveInt = 1
    pending: Publication | None = None
    check_candidate: Candidate | None = None
    last_receipt: Receipt | None = None


class WorkspaceConfig(Record):
    location: Path
    source: Path
    git_directory: Path
    binary: Path
    policy: PathPolicy
    initial: Digest
    warm_tip: Digest
