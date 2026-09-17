"""Validation deadlines live in a separate process from the coordinator."""

import os
from pathlib import Path
import subprocess
import sys

import pytest


@pytest.mark.parametrize(
    ("command", "expected"), [("raise SystemExit(7)", 7), ("import time; time.sleep(20)", 124)]
)
def test_supervisor_preserves_failure_and_enforces_deadline(
    tmp_path: Path, command: str, expected: int
) -> None:
    with (tmp_path / "lock").open("a+b") as lock:
        environment = os.environ.copy()
        environment["PYTHONPATH"] = str(Path(__file__).resolve().parents[1] / "src")
        result = subprocess.run(  # noqa: S603
            [
                sys.executable,
                "-m",
                "cowtree.supervise",
                "--timeout",
                "1",
                "--descriptor",
                str(lock.fileno()),
                "--",
                sys.executable,
                "-c",
                command,
            ],
            pass_fds=(lock.fileno(),),
            env=environment,
            capture_output=True,
            timeout=5,
            check=False,
        )
    assert result.returncode == expected
