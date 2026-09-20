"""A relocated standalone bundle publishes with no Python or Rust on PATH."""

from dataclasses import dataclass
import os
from pathlib import Path
import shutil
import subprocess
from typing import Literal

from pydantic import JsonValue, TypeAdapter
import pytest

from cowtree.metadata_types import Record
from cowtree.workspace_types import Leaf

from .test_bootstrap import source_repository


class Response(Record):
    status: Literal["ok"]
    kind: str
    value: JsonValue


@dataclass(frozen=True)
class Bundle:
    path: Path
    cwd: Path
    root: Path
    environment: dict[str, str]

    def run(self, *arguments: str, success: bool = True) -> subprocess.CompletedProcess[str]:
        result = subprocess.run(  # noqa: S603
            [str(self.path / "cowtree"), "workspace", "--root", str(self.root), *arguments],
            cwd=self.cwd,
            env=self.environment,
            capture_output=True,
            text=True,
            timeout=60,
        )
        assert (result.returncode == 0) == success, result.stderr + result.stdout
        return result


@pytest.fixture
def bundle(tmp_path: Path) -> Bundle:
    configured = os.environ.get("COWTREE_BUNDLE")
    if configured is None:
        pytest.skip("set COWTREE_BUNDLE to a built one-folder distribution")
    original = Path(configured).resolve()
    assert (original / "cowtree").is_file()
    assert (original / "cowtree-metadata").is_file()
    relocated = tmp_path / "relocated" / "cowtree"
    shutil.copytree(original, relocated, symlinks=True)
    tools = tmp_path / "tools"
    tools.mkdir()
    git = shutil.which("git")
    assert git is not None
    (tools / "git").symlink_to(git)
    environment = dict(
        os.environ,
        PATH=str(tools),
        PYTHONHOME="/missing-python",
        PYTHONPATH="/missing-modules",
        COWTREE_PACKAGE_WITNESS="unchanged",
    )
    result = Bundle(
        path=relocated, cwd=tmp_path, root=tmp_path / "workspace", environment=environment
    )
    return result


def test_relocated_bundle_checks_failures_timeouts_environment_and_publication(
    bundle: Bundle,
) -> None:
    source = source_repository(path=bundle.cwd / "source")
    bundle.run(
        "init",
        "--source",
        str(source),
        "--binary",
        str(bundle.path / "cowtree-metadata"),
        "--derived",
        "cache",
    )
    response = Response.model_validate_json(bundle.run("fork", str(bundle.cwd / "writer")).stdout)
    leaf = Leaf.model_validate_json(TypeAdapter(JsonValue).dump_json(response.value))
    identity = str(leaf.id)
    (leaf.path / "file.txt").write_bytes(b"candidate\n")
    bundle.run("capture", identity)
    bundle.run("prepare", identity)
    failure = bundle.run("check", identity, "--", "/bin/sh", "-c", "exit 7", success=False)
    assert "candidate check exited 7" in failure.stderr
    timeout = bundle.run(
        "check", identity, "--timeout", "1", "--", "/bin/sleep", "10", success=False
    )
    assert "candidate check exited 124" in timeout.stderr
    check = (
        'IFS= read -r value < file.txt; test "$value" = candidate'
        ' && test "$COWTREE_PACKAGE_WITNESS" = unchanged'
        ' && test "$PYTHONHOME" = /missing-python && test "$PYTHONPATH" = /missing-modules'
        " && ! command -v python && ! command -v python3"
        " && ! command -v cargo && ! command -v rustc"
    )
    bundle.run("check", identity, "--", "/bin/sh", "-c", check)
    bundle.run("commit", identity)
    response = Response.model_validate_json(bundle.run("fork", str(bundle.cwd / "reader")).stdout)
    reader = Leaf.model_validate_json(TypeAdapter(JsonValue).dump_json(response.value))
    assert (reader.path / "file.txt").read_bytes() == b"candidate\n"
    assert (reader.path / "cache/artifact").read_bytes() == b"warm build"
    assert (source / "file.txt").read_bytes() == b"source bytes\n"
    bundle.run("drop", identity)
    bundle.run("drop", str(reader.id))
    bundle.run("collect")
    assert Response.model_validate_json(bundle.run("list").stdout).value == []
