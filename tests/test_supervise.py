"""Validation deadlines live in a separate process from the coordinator."""

import os
from pathlib import Path
import subprocess
import sys

import pytest

from cowtree import checks
from cowtree.checks import Checks
from cowtree.workspace_types import Leaf


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


def test_supervisor_ignores_script_directory_but_preserves_child_environment(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    launcher = tmp_path / "launcher"
    launcher.mkdir()
    script = Path(checks.__file__).with_name("supervise.py")
    (launcher / "supervise.py").write_bytes(script.read_bytes())
    (launcher / "argparse.py").write_text("raise RuntimeError('script shadowed stdlib')\n")
    monkeypatch.setattr(checks, "__file__", str(launcher / "checks.py"))
    environment = tmp_path / "environment"
    environment.mkdir()
    (environment / "child_fixture.py").write_text("VALUE = 'visible'\n")
    monkeypatch.setenv("PYTHONPATH", str(environment))
    monkeypatch.setenv("COWTREE_TEST_CHILD_ENV", "preserved")
    leaf = Leaf(
        id=1,
        path=tmp_path,
        device=tmp_path.stat().st_dev,
        inode=tmp_path.stat().st_ino,
        node="0" * 64,
        git_head="0" * 40,
        origins={},
    )
    command = (
        sys.executable,
        "-c",
        "import os,sys,child_fixture; "
        "assert child_fixture.VALUE == 'visible'; "
        "assert os.environ['COWTREE_TEST_CHILD_ENV'] == 'preserved'; "
        "assert os.environ['PYTHONPATH'] == sys.argv[1]; "
        "assert not sys.flags.isolated; print('child environment preserved')",
        str(environment),
    )
    log = tmp_path / "validation.log"
    with (tmp_path / "lock").open("a+b") as lock:
        Checks.run(command=command, leaf=leaf, log=log, limits=(10, lock.fileno()))
    assert log.read_text() == "child environment preserved\n"
