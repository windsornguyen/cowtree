"""Exercise structured output through the installed command boundary."""

import errno
import json
import os
from pathlib import Path

from pydantic import TypeAdapter
import pytest

from cowtree.cli_output import Failure, Success
from cowtree.errors import CowtreeErrorCode
from cowtree.types import Command, DoctorReport, Worktree
from tests.conftest import Repository
from tests.test_cli import run_cli


@pytest.mark.parametrize("arguments", [["add", "--json"], ["doctor", "--json", "--bad"]])
def test_usage_errors_are_one_json_record(arguments: list[str], tmp_path: Path) -> None:
    res = run_cli(arguments=arguments, cwd=tmp_path)
    assert res.returncode == 2
    assert not res.stdout
    err = TypeAdapter(Failure).validate_json(res.stderr)
    assert err.status == "error"
    assert err.code is CowtreeErrorCode.INVALID_ARGUMENTS


def test_domain_errors_are_one_json_record(tmp_path: Path) -> None:
    res = run_cli(arguments=["doctor", "--json", "absent"], cwd=tmp_path)
    assert res.returncode == 1
    assert not res.stdout
    err = TypeAdapter(Failure).validate_json(res.stderr)
    assert err.code is CowtreeErrorCode.INVALID_ARGUMENTS


def test_global_json_flag_survives_subcommand_parsing(tmp_path: Path) -> None:
    res = run_cli(arguments=["--json", "doctor", str(tmp_path)], cwd=tmp_path)
    assert res.returncode in (0, 1), res.stderr
    assert not res.stderr
    report = TypeAdapter(Success).validate_json(res.stdout)
    assert report.kind is Command.DOCTOR


def test_json_lifecycle_preserves_registration(cow_repository: Repository, tmp_path: Path) -> None:
    repo = cow_repository
    target = tmp_path / 'branch\n"name'
    res = run_cli(arguments=["add", "--json", str(target)], cwd=repo.path)
    assert res.returncode == 0, res.stderr
    assert not res.stderr
    created = TypeAdapter(Success).validate_json(res.stdout)
    assert created.kind is Command.ADD
    assert isinstance(created.value, Worktree)
    assert created.value.path == target
    assert created.value.detached is True
    assert created.value.head == repo.git("rev-parse", "HEAD").stdout.strip()

    res = run_cli(arguments=["doctor", "--json", str(target)], cwd=repo.path)
    assert res.returncode == 0, res.stderr
    inspected = TypeAdapter(Success).validate_json(res.stdout)
    assert isinstance(inspected.value, DoctorReport)
    assert inspected.value.clone_tool is not None

    res = run_cli(arguments=["remove", "--json", str(target)], cwd=repo.path)
    assert res.returncode == 0, res.stderr
    assert TypeAdapter(Success).validate_json(res.stdout) == Success(
        kind=Command.REMOVE, value=None
    )
    assert not target.exists()


def test_delimited_json_path_does_not_change_error_format(tmp_path: Path) -> None:
    res = run_cli(arguments=["doctor", "--", "--json"], cwd=tmp_path)
    assert res.returncode == 1
    assert res.stderr.startswith("cowtree: invalid_arguments:")


def test_non_unicode_paths_round_trip_without_loss() -> None:
    path = Path(os.fsdecode(b"/tmp/non-utf8-\xff"))
    receipt = Success(kind=Command.ADD, value=Worktree(path=path))
    wire = receipt.json()
    assert wire.isascii()
    assert TypeAdapter(Success).validate_python(json.loads(wire)) == receipt
    failure = Failure(code=CowtreeErrorCode.INVALID_ARGUMENTS, message=str(path))
    wire = failure.json()
    assert wire.isascii()
    assert TypeAdapter(Failure).validate_python(json.loads(wire)) == failure


def test_native_json_add_preserves_filename_bytes(
    cow_repository: Repository, tmp_path: Path
) -> None:
    target = tmp_path / os.fsdecode(b"target-\xff")
    try:
        target.mkdir()
    except OSError as error:
        if error.errno == errno.EILSEQ:
            pytest.skip("filesystem rejects non-UTF8 names")
        raise
    target.rmdir()
    res = run_cli(arguments=["add", "--json", str(target)], cwd=cow_repository.path)
    assert res.returncode == 0, res.stderr
    created = TypeAdapter(Success).validate_python(json.loads(res.stdout))
    assert isinstance(created.value, Worktree)
    assert os.fsencode(created.value.path) == os.fsencode(target)
    assert (target / "file.txt").read_bytes() == b"original\n"
