"""Exercise the newcomer Cargo workflow through the installed command-line entrypoint."""

import os
from pathlib import Path
import sys
import time
from typing import Literal

from pydantic import JsonValue, TypeAdapter

from benchmarks.cargo_workspace import BuildReport, build, clone_entries, inventory, verify
from cowtree.exec import CommandRunner
from cowtree.metadata_types import Record
from cowtree.workspace_types import Leaf

from .test_bootstrap import metadata_binary
from .test_cargo_workspace import cargo_fixture


class Reply(Record):
    status: Literal["ok"]
    kind: str
    value: JsonValue


class QuickstartReceipt(Record):
    import_seconds: float
    fork_seconds: float
    inherited_entries: int
    initial: BuildReport
    unchanged: BuildReport
    edited: BuildReport


def command(store: Path, *arguments: str) -> Reply:
    selected_cli = os.environ.get("COWTREE_QUICKSTART_CLI")
    executable = (
        Path(sys.executable).parent / "cowtree" if selected_cli is None else Path(selected_cli)
    )
    repository = Path(__file__).resolve().parents[1]
    runner = CommandRunner()
    result = runner.run(
        [str(executable), "workspace", "--root", str(store), *arguments],
        env={**os.environ, "PYTHONPATH": str(repository / "src")},
    )
    reply = Reply.model_validate_json(result.stdout)
    return reply


def test_prepared_cli_leaf_reuses_cargo_cache_and_keeps_edits_private(tmp_path: Path) -> None:
    source = tmp_path / "source"
    spec = cargo_fixture(root=source)
    cargo_home = tmp_path / "seed-cargo-home"
    build(root=source, spec=spec, cargo_home=cargo_home, evidence=tmp_path / "seed")
    os.link(source / "target/debug/app", source / "target/hardlink-witness")
    before = inventory(root=source)
    expected = clone_entries(source=source, before=before, derived="target")
    store = tmp_path / "store"
    selected_binary = os.environ.get("COWTREE_QUICKSTART_BINARY")
    binary = metadata_binary() if selected_binary is None else Path(selected_binary)
    started = time.perf_counter()
    command(
        store,
        "init",
        "--source",
        str(source),
        "--binary",
        str(binary),
        "--derived",
        "target",
        "--derived-hardlinks",
        "clone",
        "--ephemeral",
        ".env",
    )
    import_seconds = time.perf_counter() - started
    started = time.perf_counter()
    reply = command(store, "fork", str(tmp_path / "intern"))
    fork_seconds = time.perf_counter() - started
    leaf = Leaf.model_validate_json(TypeAdapter(JsonValue).dump_json(reply.value))
    verify(root=leaf.path, expected=expected, cloned=True)
    initial = build(root=leaf.path, spec=spec, cargo_home=cargo_home, evidence=tmp_path / "initial")
    unchanged = build(
        root=leaf.path, spec=spec, cargo_home=cargo_home, evidence=tmp_path / "unchanged"
    )
    assert unchanged.compiled == 0
    assert unchanged.fresh > 0
    (leaf.path / "value/src/lib.rs").write_text("pub fn answer() -> u32 { 43 }\n")
    edited_spec = spec.model_copy(update={"expected_output": "43:seed"})
    edited = build(
        root=leaf.path, spec=edited_spec, cargo_home=cargo_home, evidence=tmp_path / "edited"
    )
    assert edited.compiled == 2
    verify(root=source, expected=before)
    receipt = QuickstartReceipt(
        import_seconds=import_seconds,
        fork_seconds=fork_seconds,
        inherited_entries=len(expected),
        initial=initial,
        unchanged=unchanged,
        edited=edited,
    )
    (tmp_path / "quickstart.json").write_text(receipt.model_dump_json(indent=2) + "\n")
    print(receipt.model_dump_json())
    command(store, "drop", str(leaf.id), "--force")
    assert not leaf.path.exists()
    verify(root=source, expected=before)
