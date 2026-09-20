"""The real workspace import publishes source bytes and retains private caches."""

from pathlib import Path

from pydantic import JsonValue, TypeAdapter
import pytest

from cowtree.errors import CowtreeError
from cowtree.exec import CommandRunner
from cowtree.fs import doctor
from cowtree.metadata_types import Entry, Operation, Tip
from cowtree.tree_types import PathPolicy
from cowtree.workspace import Workspace


def source_repository(path: Path) -> Path:
    assert doctor(path=path.parent).supported, "mounted integration requires native CoW"
    path.mkdir()
    runner = CommandRunner()
    for arguments in (
        ["init", "-q"],
        ["config", "user.email", "test@example.com"],
        ["config", "user.name", "Test"],
        ["config", "commit.gpgsign", "false"],
    ):
        runner.run(["git", "-C", str(path), *arguments])
    (path / "file.txt").write_bytes(b"source bytes\n")
    (path / ".gitignore").write_text("cache/\n.env\n")
    runner.run(["git", "-C", str(path), "add", "."])
    runner.run(["git", "-C", str(path), "commit", "-qm", "initial"])
    (path / "cache").mkdir()
    (path / "cache/artifact").write_bytes(b"warm build")
    (path / ".env").write_bytes(b"private environment")
    return path


def metadata_binary() -> Path:
    binary = Path(__file__).resolve().parents[1] / "target/debug/cowtree-metadata"
    assert binary.is_file(), "build the production metadata binary before mounted integration"
    return binary


def test_workspace_records_only_the_current_declaration(tmp_path: Path) -> None:
    source = source_repository(path=tmp_path / "source")
    workspace = Workspace.create(
        root=tmp_path / "workspace", source=source, binary=metadata_binary(), policy=PathPolicy()
    )
    record = TypeAdapter(dict[str, JsonValue]).validate_json(
        (workspace.root / "workspace.json").read_bytes()
    )
    assert "schema_version" not in record
    assert Workspace.open(root=workspace.root).config == workspace.config


@pytest.mark.parametrize("missing", ["trash", "receipts"])
def test_missing_workspace_layout_is_rejected_without_recreation(
    tmp_path: Path, missing: str
) -> None:
    source = source_repository(path=tmp_path / "source")
    workspace = Workspace.create(
        root=tmp_path / "workspace", source=source, binary=metadata_binary(), policy=PathPolicy()
    )
    (workspace.root / missing).rmdir()
    before = (workspace.root / "workspace.json").read_bytes()
    reopened = Workspace.open(root=workspace.root)
    with pytest.raises(CowtreeError, match="workspace layout mismatch"), reopened.session():
        pytest.fail("incomplete workspace admitted")
    assert not (workspace.root / missing).exists()
    assert (workspace.root / "workspace.json").read_bytes() == before


def test_workspace_import_connects_real_source_to_sqlite(tmp_path: Path) -> None:
    source = source_repository(path=tmp_path / "source")
    workspace = Workspace.create(
        root=tmp_path / "workspace",
        source=source,
        binary=metadata_binary(),
        policy=PathPolicy(derived=("cache",), ephemeral=(".env",)),
    )
    reopened = Workspace.open(root=workspace.root)
    node = reopened.nodes.read(identity=reopened.config.initial)
    tree = reopened.root / "nodes" / node.id / "tree"
    assert (tree / "cache/artifact").read_bytes() == b"warm build"
    assert not (tree / ".env").exists()
    with reopened.session() as metadata:
        tip = metadata.call(Operation.TIP).decode("tip", TypeAdapter(Tip))
        manifest = metadata.call(Operation.SNAPSHOT, {"version": tip.version}).decode(
            "snapshot", TypeAdapter(dict[str, Entry])
        )
        assert manifest == node.source
        assert metadata.call(Operation.LEAVES).decode("leaves", TypeAdapter(list[int])) == []
    assert (source / "file.txt").read_bytes() == b"source bytes\n"


def test_prepared_initialization_recovers_after_process_boundary(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = source_repository(path=tmp_path / "source")
    root = tmp_path / "workspace"

    from cowtree.publication import publish_directory

    def interrupt(source: Path, target: Path) -> None:
        if target == root:
            raise InterruptedError("before publication")
        publish_directory(source=source, target=target)

    with monkeypatch.context() as patch:
        patch.setattr("cowtree.workspace.publish_directory", interrupt)
        with pytest.raises(InterruptedError):
            Workspace.create(
                root=root,
                source=source,
                binary=metadata_binary(),
                policy=PathPolicy(derived=("cache",), ephemeral=(".env",)),
            )
    assert not root.exists()
    assert Workspace.recover_initialization(root=root)
    assert Workspace.recover_initialization(root=root)
    workspace = Workspace.open(root=root)
    workspace.nodes.verify(node=workspace.nodes.read(identity=workspace.config.initial))


def test_incomplete_initialization_can_be_aborted_without_changing_source(tmp_path: Path) -> None:
    source = source_repository(path=tmp_path / "source")
    root = tmp_path / "workspace"
    with pytest.raises(CowtreeError, match="metadata executable unavailable"):
        Workspace.create(root=root, source=source, binary=tmp_path / "missing", policy=PathPolicy())
    assert not Workspace.recover_initialization(root=root)
    assert not root.exists()
    assert not Workspace.staging(root=root).exists()
    assert (source / "file.txt").read_bytes() == b"source bytes\n"
