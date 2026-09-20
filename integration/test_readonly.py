"""Python callers receive the Rust authority's read-only refusal as typed data."""

import hashlib
from pathlib import Path

from pydantic import TypeAdapter
import pytest

from cowtree.initial_import import ImportProgress
from cowtree.metadata import Metadata, MetadataError
from cowtree.metadata_types import (
    Entry,
    EntryKind,
    MetadataReason,
    Operation,
    ReadOnlyKind,
    ReadOnlyScope,
    ReadOnlyScopeDetails,
    Tip,
)


def test_python_cannot_acquire_an_imported_readonly_namespace(tmp_path: Path) -> None:
    binary = Path(__file__).resolve().parents[1] / "target/debug/cowtree-metadata"
    assert binary.is_file(), "build cowtree-metadata before running integration tests"
    source = tmp_path / "source"
    (source / "vendor/dep").mkdir(parents=True)
    (source / "vendor/dep/file").write_bytes(b"dependency")
    entry = Entry(
        object=hashlib.sha256(b"dependency").hexdigest(),
        kind=ReadOnlyKind(read_only=ReadOnlyScope(file_kind=EntryKind.FILE, scope="vendor/dep")),
    )
    with Metadata(root=tmp_path / "authority", binary=binary) as store:
        store.call(Operation.INIT)
        progress = store.call(
            Operation.BEGIN_IMPORT,
            {
                "manifest": {"vendor/dep/file": entry.model_dump(mode="json")},
            },
        ).decode("import_progress", TypeAdapter(ImportProgress))
        store.call(Operation.IMPORT_CHUNK, {"root": progress.root, "source": str(source)})
        store.call(Operation.FINISH_IMPORT, {"root": progress.root})
        tip = store.call(Operation.TIP).decode("tip", TypeAdapter(Tip))
        snapshot = store.call(Operation.SNAPSHOT, {"version": tip.version}).decode(
            "snapshot",
            TypeAdapter(dict[str, Entry]),
        )
        assert snapshot == {"vendor/dep/file": entry}
        leaf = store.call(Operation.CREATE_LEAF).decode("leaf", TypeAdapter(int))
        with pytest.raises(MetadataError) as caught:
            store.call(Operation.ACQUIRE, {"leaf": leaf, "paths": ["vendor"]})
        assert caught.value.reason is MetadataReason.READ_ONLY_PATH
        assert isinstance(caught.value.details, ReadOnlyScopeDetails)
        assert caught.value.details.scope == "vendor/dep"
