"""Exercise complete standalone commands on the mounted ReFS test volume."""

import os
from pathlib import Path
import subprocess
import sys

from pydantic import TypeAdapter

from cowtree.cli_output import Failure, Success
from cowtree.errors import CowtreeErrorCode
from cowtree.exec import CommandRunner
from cowtree.types import CommandResult, Worktree


def arguments(command: list[str]) -> list[str]:
    result = [
        sys.executable,
        "-c",
        "from cowtree.cli import main; raise SystemExit(main())",
        *command,
    ]
    return result


def environment() -> dict[str, str]:
    result = {**os.environ, "PYTHONPATH": str(Path(__file__).resolve().parents[1] / "src")}
    return result


def cli(io: CommandRunner, command: list[str], cwd: Path) -> CommandResult:
    result = io.run(argv=arguments(command=command), cwd=str(cwd), env=environment(), check=False)
    return result


def repository(io: CommandRunner, root: Path) -> Path:
    source = root / "repo"
    io.run(argv=["git", "init", "-q", str(source)])
    io.run(argv=["git", "-C", str(source), "config", "user.name", "Repro"])
    io.run(argv=["git", "-C", str(source), "config", "user.email", "repro@example.invalid"])
    io.run(argv=["git", "-C", str(source), "config", "core.autocrlf", "false"])
    (source / "file.txt").write_bytes(b"source\n")
    io.run(argv=["git", "-C", str(source), "add", "file.txt"])
    io.run(argv=["git", "-C", str(source), "commit", "-qm", "fixture"])
    return source


def test_standalone_lifecycle_is_native_and_isolated(tmp_path: Path) -> None:
    io = CommandRunner()
    source = repository(io=io, root=tmp_path)
    target = tmp_path / "target"
    res = cli(io=io, command=["doctor", "--json", str(source)], cwd=source)
    assert res.returncode == 0, res.stderr
    assert "FSCTL_DUPLICATE_EXTENTS_TO_FILE" in res.stdout
    res = cli(io=io, command=["add", "--json", "-b", "branch", str(target)], cwd=source)
    assert res.returncode == 0, res.stderr
    created = TypeAdapter(Success).validate_json(res.stdout)
    assert isinstance(created.value, Worktree)
    assert created.value.branch == "refs/heads/branch"
    (target / "file.txt").write_bytes(b"target\n")
    assert (source / "file.txt").read_bytes() == b"source\n"
    res = cli(io=io, command=["remove", "--json", str(target)], cwd=source)
    assert res.returncode == 1
    assert target.exists()
    res = cli(io=io, command=["remove", "--json", "--force", str(target)], cwd=source)
    assert res.returncode == 0, res.stderr
    assert not target.exists()
    assert io.run(argv=["git", "-C", str(source), "rev-parse", "branch"]).stdout


def test_concurrent_forks_and_removals_serialize(tmp_path: Path) -> None:
    io = CommandRunner()
    source = repository(io=io, root=tmp_path)
    targets = [tmp_path / "first", tmp_path / "second"]
    for operation in ("add", "remove"):
        children = [
            subprocess.Popen(  # noqa: S603 -- invoke only the current interpreter and test source CLI
                arguments(command=[operation, str(target)]),
                cwd=source,
                env=environment(),
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            for target in targets
        ]
        try:
            for child in children:
                _, error = child.communicate(timeout=30)
                assert child.returncode == 0, error.decode()
        finally:
            for child in children:
                if child.poll() is None:
                    child.kill()
                child.wait(timeout=30)
    assert all(not target.exists() for target in targets)


def test_managed_commands_fail_before_creating_state(tmp_path: Path) -> None:
    store = tmp_path / "store"
    res = cli(io=CommandRunner(), command=["workspace", "--root", str(store), "init"], cwd=tmp_path)
    assert res.returncode == 1
    error = TypeAdapter(Failure).validate_json(res.stderr)
    assert error.code is CowtreeErrorCode.COW_UNAVAILABLE
    assert "managed workspaces" in error.message
    assert not store.exists()
