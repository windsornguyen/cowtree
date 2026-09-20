from __future__ import annotations

import errno
import json
import os
from pathlib import Path
import subprocess
import sys

import pytest

from cowtree.core import inspect_path, list_all_worktrees
from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.exec import CommandRunner
from cowtree.types import WorktreeAddRequest


def run_cli(arguments: list[str], cwd: Path) -> subprocess.CompletedProcess[str]:
    binary = os.environ.get("COWTREE_TEST_BINARY")
    command = (
        [binary, *arguments]
        if binary is not None
        else [
            sys.executable,
            "-c",
            "from cowtree.cli import main; raise SystemExit(main())",
            *arguments,
        ]
    )
    environment = {**os.environ, "PYTHONPATH": str(Path(__file__).resolve().parents[1] / "src")}
    result = subprocess.run(  # noqa: S603
        command,
        cwd=cwd,
        env=environment,
        capture_output=True,
        text=True,
        errors="surrogateescape",
        check=False,
    )
    return result


@pytest.mark.parametrize(
    "arguments",
    [
        ["wat"],
        ["doctor", ".", "ignored"],
        ["doctor", "--unknown"],
        ["help", "ignored"],
        ["list", ".", "ignored"],
        ["list", "--j"],
        ["list", "--unknown"],
        ["remove"],
        ["remove", "one", "two"],
        ["remove", "--unknown", "one"],
        ["remove", "-f", "one"],
        ["add"],
        ["add", "--force", "target"],
        ["add", "-B", "existing", "target"],
        ["add", "--orphan", "target"],
        ["add", "--track", "target"],
        ["add", "--no-track", "target"],
        ["add", "--guess-remote", "target"],
        ["add", "-b", "new", "--detach", "target"],
        ["add", "--reason", "unlocked", "target"],
        ["add", "--lock", "--reason", "", "target"],
        ["add", "-b", "", "target"],
        ["add", "target", ""],
        ["add", "target", "HEAD", "ignored"],
        ["add", "--reason"],
        ["add", "--unknown", "target"],
        ["add", "--det", "target"],
        ["add", "--cow", "target"],
        ["add", "--no-checkout", "target"],
    ],
)
def test_malformed_cli_arguments_fail(arguments: list[str], tmp_path: Path) -> None:
    result = run_cli(arguments=arguments, cwd=tmp_path)
    assert result.returncode == 2
    assert "usage:" in result.stderr.lower() or "--help" in result.stderr
    assert "Traceback" not in result.stderr


@pytest.mark.parametrize(
    "arguments",
    [
        [],
        ["help"],
        ["-h"],
        ["--help"],
        ["add", "--help"],
        ["list", "-h"],
        ["remove", "--help"],
        ["doctor", "-h"],
    ],
)
def test_help_succeeds_without_a_repository(arguments: list[str], tmp_path: Path) -> None:
    result = run_cli(arguments=arguments, cwd=tmp_path)
    assert result.returncode == 0
    assert "usage: cowtree" in result.stdout.lower()
    assert not result.stderr


@pytest.mark.parametrize(
    "arguments", [["list"], ["add", "target"], ["remove", "missing"], ["doctor", "missing"]]
)
def test_domain_failures_return_one(arguments: list[str], tmp_path: Path) -> None:
    result = run_cli(arguments=arguments, cwd=tmp_path)
    assert result.returncode == 1
    assert "cowtree:" in result.stderr
    assert "Traceback" not in result.stderr


def create_repository(path: Path) -> None:
    runner = CommandRunner()
    runner.run(argv=["git", "init", "-q", str(path)])
    runner.run(argv=["git", "-C", str(path), "config", "user.email", "test@example.com"])
    runner.run(argv=["git", "-C", str(path), "config", "user.name", "CLI test"])
    runner.run(argv=["git", "-C", str(path), "config", "commit.gpgsign", "false"])
    runner.run(argv=["git", "-C", str(path), "config", "core.autocrlf", "false"])
    (path / "payload\n東京.txt").write_bytes(b"payload\x00\xff\n")
    runner.run(argv=["git", "-C", str(path), "add", "."])
    runner.run(argv=["git", "-C", str(path), "commit", "-qm", "fixture"])


@pytest.mark.parametrize(
    "arguments", [["add", "loop/child"], ["list", "loop"], ["remove", "loop"], ["doctor", "loop"]]
)
def test_cli_reports_symlink_loops_as_domain_failures(arguments: list[str], tmp_path: Path) -> None:
    source = tmp_path / "source"
    create_repository(path=source)
    (source / "loop").symlink_to("loop")
    result = run_cli(arguments=arguments, cwd=source)
    assert result.returncode == 1
    assert "cowtree:" in result.stderr
    assert "Traceback" not in result.stderr


def test_cli_preserves_paths_locks_and_delimiters(tmp_path: Path) -> None:
    report = inspect_path(path=tmp_path)
    if not report.supported:
        pytest.skip(f"CoW unsupported: {report.reason}")
    source = tmp_path / 'repo\n"東京'
    create_repository(path=source)
    target_name = '-target\n"東京'
    target = source / target_name
    reason = 'held\n"for testing"'

    result = run_cli(
        arguments=["add", "-b", "cli-lock", "--lock", "--reason", reason, "--", target_name],
        cwd=source,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout == f"{target}\n"
    assert (target / "payload\n東京.txt").read_bytes() == b"payload\x00\xff\n"
    worktrees = list_all_worktrees(source=source)
    added = next(worktree for worktree in worktrees if worktree.path == target)
    assert added.locked is True
    assert added.reason == reason
    result = run_cli(arguments=["list", "--json", "--", str(source)], cwd=tmp_path)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == [
        json.loads(worktree.to_json_text()) for worktree in worktrees
    ]

    result = run_cli(arguments=["remove", "--", target_name], cwd=source)
    assert result.returncode == 1
    assert target.is_dir()
    runner = CommandRunner()
    runner.run(argv=["git", "-C", str(source), "worktree", "unlock", str(target)])
    result = run_cli(arguments=["remove", "--", target_name], cwd=source)
    assert result.returncode == 0, result.stderr
    assert not target.exists()


@pytest.mark.parametrize("flags", [[], ["--detach"], ["-d"]])
def test_cli_default_and_explicit_detach_match(flags: list[str], tmp_path: Path) -> None:
    report = inspect_path(path=tmp_path)
    if not report.supported:
        pytest.skip(f"CoW unsupported: {report.reason}")
    source = tmp_path / "source"
    create_repository(path=source)
    target = tmp_path / "target"
    result = run_cli(arguments=["add", *flags, str(target), "HEAD"], cwd=source)
    assert result.returncode == 0, result.stderr
    added = next(
        worktree for worktree in list_all_worktrees(source=source) if worktree.path == target
    )
    assert added.detached is True
    assert added.branch is None
    result = run_cli(arguments=["remove", str(target)], cwd=source)
    assert result.returncode == 0, result.stderr


def test_cli_round_trips_non_utf8_paths_when_supported(tmp_path: Path) -> None:
    source = tmp_path / os.fsdecode(b"source-\xff")
    try:
        source.mkdir()
    except OSError as error:
        if error.errno == errno.EILSEQ or (sys.platform == "darwin" and error.errno == errno.EPERM):
            pytest.skip("filesystem rejects non-UTF8 filenames")
        raise
    create_repository(path=source)
    result = run_cli(arguments=["list", "--json", "--", str(source)], cwd=tmp_path)
    assert result.returncode == 0, result.stderr
    worktrees = list_all_worktrees(source=source)
    assert json.loads(result.stdout) == [
        json.loads(worktree.to_json_text()) for worktree in worktrees
    ]
    assert worktrees[0].path == source


@pytest.mark.parametrize(
    ("branch", "detach", "lock", "reason", "commitish"),
    [
        ("branch", True, False, None, "HEAD"),
        (None, False, False, "unlocked", "HEAD"),
        ("", False, False, None, "HEAD"),
        ("nul\0branch", False, False, None, "HEAD"),
        (None, False, True, "", "HEAD"),
        (None, False, True, "nul\0reason", "HEAD"),
        (None, False, False, None, ""),
        (None, False, False, None, "HEAD\0"),
    ],
)
def test_request_rejects_invalid_states(
    branch: str | None, detach: bool, lock: bool, reason: str | None, commitish: str
) -> None:
    with pytest.raises(CowtreeError) as raised:
        WorktreeAddRequest(
            path=Path("target"),
            branch=branch,
            detach=detach,
            lock=lock,
            reason=reason,
            commitish=commitish,
        )
    assert raised.value.code is CowtreeErrorCode.INVALID_ARGUMENTS
