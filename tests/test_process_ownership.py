"""A child keeps its operation lock when the coordinating process dies."""

import fcntl
import os
from pathlib import Path
import subprocess
import sys
import time

import pytest


def test_child_retains_operation_lock_after_controller_exit(tmp_path: Path) -> None:
    lock = tmp_path / "lock"
    started = tmp_path / "started"
    release = tmp_path / "release"
    child = (
        "import pathlib,sys,time; "
        "pathlib.Path(sys.argv[1]).touch(); deadline=time.monotonic()+10\n"
        "while not pathlib.Path(sys.argv[2]).exists() and time.monotonic()<deadline:\n"
        " time.sleep(.01)\n"
    )
    controller = (
        "import fcntl,sys\nfrom cowtree.exec import CommandRunner\n"
        "stream=open(sys.argv[1],'a+b')\nfcntl.flock(stream,fcntl.LOCK_EX)\n"
        "CommandRunner(descriptors=(stream.fileno(),)).run([sys.executable,'-c',sys.argv[2],sys.argv[3],sys.argv[4]])\n"
    )
    environment = os.environ.copy()
    environment["PYTHONPATH"] = str(Path(__file__).resolve().parents[1] / "src")
    process = subprocess.Popen(  # noqa: S603
        [sys.executable, "-c", controller, str(lock), child, str(started), str(release)],
        env=environment,
    )
    try:
        deadline = time.monotonic() + 5
        while not started.exists() and time.monotonic() < deadline:
            time.sleep(0.01)
        assert started.exists()
        process.kill()
        process.wait(timeout=5)
        with lock.open("rb") as stream:
            with pytest.raises(BlockingIOError):
                fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
            release.touch()
            deadline = time.monotonic() + 5
            while True:
                try:
                    fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    break
                except BlockingIOError:
                    if time.monotonic() >= deadline:
                        raise
                    time.sleep(0.01)
    finally:
        release.touch()
        if process.poll() is None:
            process.kill()
        process.wait(timeout=5)
