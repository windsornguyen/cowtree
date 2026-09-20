"""Validated records exchanged with the local Rust metadata authority."""

from __future__ import annotations

from enum import Enum
import json
from typing import Annotated, Literal, TypeVar

from pydantic import (
    BaseModel,
    ConfigDict,
    Field,
    JsonValue,
    PositiveInt,
    StringConstraints,
    TypeAdapter,
)


Digest = Annotated[str, StringConstraints(pattern=r"^[0-9a-f]{64}$", min_length=64, max_length=64)]
Value = TypeVar("Value")


class Record(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, strict=True)


class EntryKind(str, Enum):
    FILE = "file"
    EXECUTABLE = "executable"
    SYMLINK = "symlink"


class Entry(Record):
    object: Digest
    kind: EntryKind


class Grant(Record):
    leaf: PositiveInt
    path: str
    token: PositiveInt
    origin: Entry | None
    activated: bool


class Request(Record):
    leaf: PositiveInt
    sequence: PositiveInt


class Candidate(Record):
    request: Request
    attempt: PositiveInt
    parent: Annotated[int, Field(ge=0)]
    root: Digest


class CapturedChange(Record):
    path: str
    token: PositiveInt
    origin: Entry | None
    value: Entry | None
    edit_revision: Annotated[int, Field(ge=0)]


class Proposal(Record):
    request: Request
    captured_at: Annotated[int, Field(ge=0)]
    changes: list[CapturedChange]


class Receipt(Record):
    request: Request
    version: Annotated[int, Field(ge=0)]
    root: Digest


class BatchCandidate(Record):
    members: list[Candidate]


class BatchReceipt(Record):
    receipts: list[Receipt]


class Tip(Record):
    version: Annotated[int, Field(ge=0)]
    root: Digest


class Operation(str, Enum):
    INIT = "init"
    BEGIN_IMPORT = "begin_import"
    IMPORT_CHUNK = "import_chunk"
    FINISH_IMPORT = "finish_import"
    STATUS_IMPORT = "status_import"
    CREATE_LEAF = "create_leaf"
    LEAVES = "leaves"
    GRANTS = "grants"
    TIP = "tip"
    SNAPSHOT = "snapshot"
    READ = "read"
    ACQUIRE = "acquire"
    ACTIVATE = "activate"
    STAGE = "stage"
    STAGE_FILE = "stage_file"
    EDIT = "edit"
    VIEW = "view"
    RELEASE = "release"
    REVOKE = "revoke"
    DISCARD = "discard"
    DISCARD_UPLOAD = "discard_upload"
    DROP_LEAF = "drop_leaf"
    PROPOSE = "propose"
    PREPARE = "prepare"
    PREPARE_BATCH = "prepare_batch"
    COMMIT_BATCH = "commit_batch"
    RESOLVE = "resolve"
    COMMIT = "commit"
    RESULT = "result"
    ABORT = "abort"
    RETAIN = "retain"
    RELEASE_RETENTION = "release_retention"
    MAINTAIN = "maintain"
    REPLACE_CLIENT_PINS = "replace_client_pins"


class Output(Record):
    kind: str
    value: JsonValue = None

    def decode(self, kind: str, adapter: TypeAdapter[Value]) -> Value:
        """Check the response tag before interpreting its payload."""
        if self.kind != kind:
            raise ValueError(f"expected metadata output {kind}, received {self.kind}")
        value = adapter.validate_json(json.dumps(self.value), strict=True)
        return value


class Success(Record):
    status: Literal["ok"]
    output: Output


class MetadataReason(str, Enum):
    INVALID_REQUEST = "invalid_request"
    SQLITE = "sqlite"
    DATABASE_BUSY = "database_busy"
    OBJECT_IDENTIFIER = "object_identifier"
    OBJECT_MISSING = "object_missing"
    OBJECT_CORRUPT = "object_corrupt"
    OBJECT_UNEXPECTED_ENTRY = "object_unexpected_entry"
    OBJECT_IO = "object_io"
    METADATA_JSON = "metadata_json"
    IO = "io"
    INVALID_PATH = "invalid_path"
    INVALID_INPUT_FILE = "invalid_input_file"
    IMPORT_CONFLICT = "import_conflict"
    IMPORT_NOT_READY = "import_not_ready"
    IMPORT_SOURCE_CHANGED = "import_source_changed"
    INVALID_ID = "invalid_id"
    SCHEMA = "schema"
    SQLITE_VERSION = "sqlite_version"
    DURABILITY = "durability"
    LEAF_INACTIVE = "leaf_inactive"
    LEASE_CONFLICT = "lease_conflict"
    STALE_TOKEN = "stale_token"  # noqa: S105 - protocol error category, not a credential.
    NOT_ACTIVATED = "not_activated"
    DIRTY_PATH = "dirty_path"
    UPLOAD_NOT_READY = "upload_not_ready"
    REQUEST_EXPIRED = "request_expired"
    REQUEST_SEQUENCE = "request_sequence"
    REQUEST_CONFLICT = "request_conflict"
    EMPTY_PROPOSAL = "empty_proposal"
    ABORTED = "aborted"
    CANDIDATE_MISMATCH = "candidate_mismatch"
    CANDIDATE_NOT_READY = "candidate_not_ready"
    TIP_CHANGED = "tip_changed"
    STALE_ORIGIN = "stale_origin"
    SNAPSHOT_EXPIRED = "snapshot_expired"
    NAMESPACE_CONFLICT = "namespace_conflict"
    INVALID_SYMLINK = "invalid_symlink"
    LIMIT_EXCEEDED = "limit_exceeded"
    BATCH_CONFLICT = "batch_conflict"
    RESOLUTION_CONFLICT = "resolution_conflict"
    COUNTER_EXHAUSTED = "counter_exhausted"
    CHECKPOINT_BUSY = "checkpoint_busy"


class RetryAction(str, Enum):
    NONE = "none"
    RETRY_SAME_REQUEST = "retry_same_request"
    REPREPARE = "reprepare"
    RUN_MAINTENANCE = "run_maintenance"
    RESOLVE_CONFLICT = "resolve_conflict"


class NoDetails(Record):
    kind: Literal["none"]


class ConflictDetails(Record):
    kind: Literal["conflict"]
    reason: str


class PathDetails(Record):
    kind: Literal["path"]
    path: str


class IdentifierDetails(Record):
    kind: Literal["identifier"]
    value: int


class LeafDetails(Record):
    kind: Literal["leaf"]
    leaf: int


class SequenceDetails(Record):
    kind: Literal["sequence"]
    sequence: int


class MismatchDetails(Record):
    kind: Literal["sequence_mismatch", "tip"]
    expected: int
    actual: int


class VersionDetails(Record):
    kind: Literal["version"]
    version: int


class SqliteDetails(Record):
    kind: Literal["sqlite"]
    extended_code: int | None


class RequiredSqliteDetails(Record):
    kind: Literal["required_sqlite"]
    actual: str


class LimitDetails(Record):
    kind: Literal["limit"]
    resource: Literal[
        "active_leaves",
        "pending_uploads",
        "pending_proposals",
        "retained_paths",
        "snapshot_paths",
        "object_bytes",
        "metadata_history",
        "manual_pins",
        "wal_bytes",
        "configuration",
    ]


class ObjectIdentifierDetails(Record):
    kind: Literal["object_identifier"]
    value: str


class ObjectMissingDetails(Record):
    kind: Literal["object_missing"]
    path: str
    object: str


class ObjectCorruptDetails(Record):
    kind: Literal["object_corrupt"]
    path: str
    expected: str
    actual: str


class IoDetails(Record):
    kind: Literal["io"]
    path: str
    os_code: int | None


class InvalidRequestDetails(Record):
    kind: Literal["invalid_request"]
    line: int
    column: int


class RequestTooLargeDetails(Record):
    kind: Literal["request_too_large"]
    max_bytes: int


ErrorDetails = Annotated[
    NoDetails
    | ConflictDetails
    | PathDetails
    | IdentifierDetails
    | LeafDetails
    | SequenceDetails
    | MismatchDetails
    | VersionDetails
    | SqliteDetails
    | RequiredSqliteDetails
    | LimitDetails
    | ObjectIdentifierDetails
    | ObjectMissingDetails
    | ObjectCorruptDetails
    | IoDetails
    | InvalidRequestDetails
    | RequestTooLargeDetails,
    Field(discriminator="kind"),
]


class Failure(Record):
    status: Literal["error"]
    code: MetadataReason
    message: str
    retry_action: RetryAction
    details: ErrorDetails


Response = Annotated[Success | Failure, Field(discriminator="status")]
