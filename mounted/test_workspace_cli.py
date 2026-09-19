"""The public CLI completes a warm, checked publication without Python glue."""

import json
from pathlib import Path
import sys
from typing import Literal

from pydantic import JsonValue
import pytest

from cowtree.captures import Captures
from cowtree.cli import main
from cowtree.metadata_types import Record
from cowtree.workspace_cli import parse
from cowtree.workspace_types import Leaf

from .test_bootstrap import metadata_binary, source_repository
from .test_fork import manager


class Result(Record):
    status: Literal["ok"]
    kind: str
    value: JsonValue


def test_cli_completes_warm_publication(tmp_path: Path, capsys: pytest.CaptureFixture[str]) -> None:
    source = source_repository(path=tmp_path / "source")
    root = tmp_path / "workspace"

    def run(*arguments: str) -> Result:
        assert main(["workspace", "--root", str(root), *arguments]) == 0
        result = Result.model_validate_json(capsys.readouterr().out)
        return result

    run("init", "--source", str(source), "--binary", str(metadata_binary()), "--derived", "cache")
    leaf = Leaf.model_validate_json(json.dumps(run("fork", str(tmp_path / "writer")).value))
    identity = str(leaf.id)
    target = leaf.path
    assert (target / "cache/artifact").read_bytes() == b"warm build"
    assert not (target / ".env").exists()
    (target / "file.txt").write_bytes(b"candidate")
    run("capture", identity)
    run("prepare", identity)
    run("check", identity, "--", sys.executable, "-c", "pass")
    (target / "file.txt").write_bytes(b"later edit")
    run("commit", identity)
    assert (target / "file.txt").read_bytes() == b"later edit"
    reader = Leaf.model_validate_json(json.dumps(run("fork", str(tmp_path / "reader")).value))
    assert (reader.path / "file.txt").read_bytes() == b"candidate"
    run("seal", identity)
    run("sync", str(reader.id))
    run("recover")
    run("drop", identity, "--force")
    run("collect")
    remaining = run("list").value
    assert isinstance(remaining, list)
    assert len(remaining) == 1


def test_cli_reports_missing_workspace_as_json(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    assert main(["workspace", "--root", str(tmp_path / "missing"), "list"]) == 1
    result = json.loads(capsys.readouterr().err)
    assert result["status"] == "error"
    assert result["code"] == "io"


def test_check_options_after_identity_preserve_the_child_argv(tmp_path: Path) -> None:
    options = parse(
        argv=[
            "--root",
            str(tmp_path),
            "check",
            "2",
            "--timeout",
            "7",
            "--",
            "python",
            "-c",
            "pass",
        ],
    )
    assert options.timeout == 7
    assert options.command == ["python", "-c", "pass"]


def test_cli_checks_the_exact_batch_file(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    leaves = manager(root=tmp_path)
    first = leaves.fork(path=tmp_path / "first")
    second = leaves.fork(path=tmp_path / "second")
    (first.path / "left.txt").write_text("left")
    (second.path / "right.txt").write_text("right")
    captures = Captures(leaves.workspace)
    captures.capture(identity=first.id)
    captures.capture(identity=second.id)
    prefix = ["workspace", "--root", str(leaves.workspace.root)]
    assert main([*prefix, "prepare-batch", str(first.id), str(second.id)]) == 0
    prepared = Result.model_validate_json(capsys.readouterr().out)
    candidate = tmp_path / "candidate.json"
    candidate.write_text(json.dumps(prepared.value))
    assert (
        main(
            [
                *prefix,
                "check-batch",
                "--candidate",
                str(candidate),
                "--timeout",
                "10",
                "--",
                sys.executable,
                "-c",
                "from pathlib import Path; assert Path('left.txt').read_text()=='left'; "
                "assert Path('right.txt').read_text()=='right'",
            ]
        )
        == 0
    )
    capsys.readouterr()
    assert main([*prefix, "commit-batch", "--candidate", str(candidate)]) == 0
    capsys.readouterr()
    assert main([*prefix, "commit-batch", "--candidate", str(candidate)]) == 0


def test_invalid_usage_is_structured(capsys: pytest.CaptureFixture[str]) -> None:
    assert main(["workspace", "--root", "/unused", "unknown"]) == 2
    result = json.loads(capsys.readouterr().err)
    assert result["code"] == "invalid_arguments"


def test_log_lists_only_acknowledged_nodes(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    leaves = manager(root=tmp_path)
    partial = leaves.workspace.root / "nodes" / ("0" * 64)
    partial.mkdir()
    assert main(["workspace", "--root", str(leaves.workspace.root), "log"]) == 0
    result = Result.model_validate_json(capsys.readouterr().out)
    assert isinstance(result.value, list)
    assert len(result.value) == 1
    assert partial.exists()
