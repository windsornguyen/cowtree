"""Typed values crossing the workspace metadata process boundary."""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import TypeAlias


Json: TypeAlias = str | int | float | bool | None | list["Json"] | dict[str, "Json"]


class RetryAction(str, Enum):
    NONE = "none"
    RETRY_SAME_REQUEST = "retry_same_request"
    REPREPARE = "reprepare"
    RUN_MAINTENANCE = "run_maintenance"
    RESOLVE_CONFLICT = "resolve_conflict"
    RECOVER_INSTALLATION = "recover_installation"
    SYNC_WORKSPACE = "sync_workspace"


class MetadataError(RuntimeError):
    """Preserve machine-readable authority errors without parsing their prose."""

    def __init__(
        self,
        code: str,
        message: str,
        retry_action: RetryAction = RetryAction.NONE,
        details: WireRecord | None = None,
    ) -> None:
        super().__init__(message)
        self.code = code
        self.message = message
        self.retry_action = retry_action
        self.details = details


@dataclass(frozen=True)
class WireRecord:
    """Validate fields from a JSON object before constructing public values."""

    fields: dict[str, Json]

    @classmethod
    def parse(cls, value: Json) -> WireRecord:
        if not isinstance(value, dict):
            raise MetadataError("invalid_response", "expected a JSON object")
        result = cls(fields=value)
        return result

    def value(self, key: str) -> Json:
        if key not in self.fields:
            raise MetadataError("invalid_response", f"missing field: {key}")
        result = self.fields[key]
        return result

    def text(self, key: str) -> str:
        result = self.value(key)
        if not isinstance(result, str):
            raise MetadataError("invalid_response", f"expected string: {key}")
        return result

    def number(self, key: str) -> int:
        result = self.value(key)
        if type(result) is not int:
            raise MetadataError("invalid_response", f"expected integer: {key}")
        if result < 0:
            raise MetadataError("invalid_response", f"expected unsigned integer: {key}")
        return result

    def flag(self, key: str) -> bool:
        result = self.value(key)
        if not isinstance(result, bool):
            raise MetadataError("invalid_response", f"expected boolean: {key}")
        return result

    def record(self, key: str) -> WireRecord:
        result = WireRecord.parse(self.value(key))
        return result


class EntryKind(str, Enum):
    FILE = "file"
    EXECUTABLE = "executable"
    SYMLINK = "symlink"


@dataclass(frozen=True)
class Entry:
    object: str
    kind: EntryKind

    @classmethod
    def parse(cls, value: Json) -> Entry | None:
        if value is None:
            return None
        row = WireRecord.parse(value)
        try:
            kind = EntryKind(row.text("kind"))
        except ValueError as error:
            raise MetadataError("invalid_response", "unknown entry kind") from error
        result = cls(object=row.text("object"), kind=kind)
        return result

    def wire(self) -> Json:
        result: Json = {"object": self.object, "kind": self.kind.value}
        return result


@dataclass(frozen=True)
class Grant:
    leaf: int
    path: str
    token: int
    origin: Entry | None
    activated: bool

    @classmethod
    def parse(cls, value: Json) -> Grant:
        row = WireRecord.parse(value)
        result = cls(
            leaf=row.number("leaf"),
            path=row.text("path"),
            token=row.number("token"),
            origin=Entry.parse(row.value("origin")),
            activated=row.flag("activated"),
        )
        return result

    def wire(self) -> Json:
        result: Json = {
            "leaf": self.leaf,
            "path": self.path,
            "token": self.token,
            "origin": None if self.origin is None else self.origin.wire(),
            "activated": self.activated,
        }
        return result


@dataclass(frozen=True)
class RequestId:
    leaf: int
    sequence: int

    @classmethod
    def parse(cls, value: Json) -> RequestId:
        row = WireRecord.parse(value)
        result = cls(leaf=row.number("leaf"), sequence=row.number("sequence"))
        return result

    def wire(self) -> Json:
        result: Json = {"leaf": self.leaf, "sequence": self.sequence}
        return result


@dataclass(frozen=True)
class Tip:
    version: int
    root: str

    @classmethod
    def parse(cls, value: Json) -> Tip:
        row = WireRecord.parse(value)
        result = cls(version=row.number("version"), root=row.text("root"))
        return result


@dataclass(frozen=True)
class Candidate:
    request: RequestId
    attempt: int
    parent: int
    root: str

    @classmethod
    def parse(cls, value: Json) -> Candidate:
        row = WireRecord.parse(value)
        result = cls(
            request=RequestId.parse(row.value("request")),
            attempt=row.number("attempt"),
            parent=row.number("parent"),
            root=row.text("root"),
        )
        return result

    def wire(self) -> Json:
        result: Json = {
            "request": self.request.wire(),
            "attempt": self.attempt,
            "parent": self.parent,
            "root": self.root,
        }
        return result


@dataclass(frozen=True)
class Receipt:
    request: RequestId
    version: int
    root: str

    @classmethod
    def parse(cls, value: Json) -> Receipt:
        row = WireRecord.parse(value)
        result = cls(
            request=RequestId.parse(row.value("request")),
            version=row.number("version"),
            root=row.text("root"),
        )
        return result


@dataclass(frozen=True)
class LeafView:
    path: str
    origin: Entry | None
    value: Entry | None
    edit_revision: int

    @classmethod
    def parse(cls, value: Json) -> LeafView:
        row = WireRecord.parse(value)
        result = cls(
            path=row.text("path"),
            origin=Entry.parse(row.value("origin")),
            value=Entry.parse(row.value("value")),
            edit_revision=row.number("edit_revision"),
        )
        return result

    @property
    def dirty(self) -> bool:
        result = self.value != self.origin
        return result


@dataclass(frozen=True)
class WorkspaceLeaf:
    leaf: int
    path: Path


def array(value: Json) -> list[Json]:
    if not isinstance(value, list):
        raise MetadataError("invalid_response", "expected a JSON array")
    return value


@dataclass(frozen=True)
class Installation:
    """Last installed epoch and any durable interrupted-install target."""

    leaf: int
    path: Path
    version: int
    pending: int | None

    @classmethod
    def parse(cls, value: Json) -> Installation:
        row = WireRecord.parse(value)
        pending = row.value("pending")
        if pending is not None:
            pending = row.number("pending")
        result = cls(
            leaf=row.number("leaf"),
            path=Path(row.text("path")),
            version=row.number("version"),
            pending=pending,
        )
        return result


@dataclass(frozen=True)
class ProposedChange:
    path: str
    token: int
    origin: Entry | None
    value: Entry | None
    edit_revision: int

    @classmethod
    def parse(cls, value: Json) -> ProposedChange:
        row = WireRecord.parse(value)
        result = cls(
            path=row.text("path"),
            token=row.number("token"),
            origin=Entry.parse(row.value("origin")),
            value=Entry.parse(row.value("value")),
            edit_revision=row.number("edit_revision"),
        )
        return result


@dataclass(frozen=True)
class Proposal:
    request: RequestId
    captured_at: int
    changes: tuple[ProposedChange, ...]

    @classmethod
    def parse(cls, value: Json) -> Proposal:
        row = WireRecord.parse(value)
        result = cls(
            request=RequestId.parse(row.value("request")),
            captured_at=row.number("captured_at"),
            changes=tuple(ProposedChange.parse(item) for item in array(row.value("changes"))),
        )
        return result


@dataclass(frozen=True)
class BatchCandidate:
    """Complete membership prepared against one parent and immutable snapshot."""

    members: tuple[Candidate, ...]

    @classmethod
    def parse(cls, value: Json) -> BatchCandidate:
        row = WireRecord.parse(value)
        result = cls(members=tuple(Candidate.parse(item) for item in array(row.value("members"))))
        return result

    def wire(self) -> Json:
        result: Json = {"members": [member.wire() for member in self.members]}
        return result


@dataclass(frozen=True)
class BatchReceipt:
    """One durable receipt per member, all naming the same committed epoch."""

    receipts: tuple[Receipt, ...]

    @classmethod
    def parse(cls, value: Json) -> BatchReceipt:
        row = WireRecord.parse(value)
        result = cls(receipts=tuple(Receipt.parse(item) for item in array(row.value("receipts"))))
        return result
