"""Authority policy survives JSON round trips without being confused with file metadata."""

import hashlib
import json
from pathlib import Path

import pytest

from cowtree import install
from cowtree.metadata_types import (
    Entry,
    EntryKind,
    Failure,
    ReadOnlyKind,
    ReadOnlyScope,
    ReadOnlyScopeDetails,
)
from tests.conftest import Repository


@pytest.mark.parametrize("kind", list(EntryKind))
def test_readonly_entries_preserve_policy_and_physical_identity(kind: EntryKind) -> None:
    wire = json.dumps(
        {
            "object": "a" * 64,
            "kind": {"read_only": {"file_kind": kind.value, "scope": "vendor/dep"}},
        }
    )
    readonly = Entry.model_validate_json(wire)
    writable = Entry(object="a" * 64, kind=kind)
    assert readonly != writable
    assert readonly.file_kind is kind
    assert install.same_image(readonly, writable)
    assert not install.same_image(readonly, None)
    assert not install.same_image(None, readonly)
    assert install.same_image(None, None)
    assert not install.same_image(readonly, Entry(object="b" * 64, kind=kind))
    assert Entry.model_validate_json(readonly.model_dump_json()) == readonly


@pytest.mark.parametrize("code", ["read_only_path", "invalid_read_only_scope"])
def test_readonly_failures_retain_typed_scope_context(code: str) -> None:
    wire = json.dumps(
        {
            "status": "error",
            "code": code,
            "message": "read-only namespace",
            "retry_action": "none",
            "details": {"kind": "read_only_scope", "path": "vendor", "scope": "vendor/dep"},
        }
    )
    failure = Failure.model_validate_json(wire)
    assert failure.code.value == code
    assert isinstance(failure.details, ReadOnlyScopeDetails)
    assert failure.details.scope == "vendor/dep"


@pytest.mark.parametrize("kind", list(EntryKind))
def test_installation_retains_readonly_policy_across_replay(
    cow_repository: Repository, tmp_path: Path, kind: EntryKind
) -> None:
    root = cow_repository.path
    objects = tmp_path / "objects"
    objects.mkdir()
    data = b"target" if kind is EntryKind.SYMLINK else b"payload"
    digest = hashlib.sha256(data).hexdigest()
    (objects / digest).write_bytes(data)
    entry = Entry(
        object=digest,
        kind=ReadOnlyKind(read_only=ReadOnlyScope(file_kind=kind, scope="protected")),
    )
    directory = tmp_path / "install"
    operation = install.Installation.prepare(
        directory=directory,
        record=install.InstallRecord(
            root=root, changes=[install.Change(path="protected/file", before=None, after=entry)]
        ),
        objects=objects,
    )
    operation.apply()
    reopened = install.Installation.open(directory)
    assert reopened.record.changes[0].after == entry
    reopened.apply()
    assert install.same_image(install.fingerprint(root / "protected/file"), entry)
    reopened.apply(rollback=True)
    assert install.fingerprint(root / "protected/file") is None
    reopened.finish()
