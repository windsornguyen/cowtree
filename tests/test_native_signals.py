"""Python interrupts must cross the binding before native creation commits."""

import os
from pathlib import Path
import shutil
import sys

import pytest

from cowtree.core import add_worktree
from tests.conftest import Repository


@pytest.mark.skipif(sys.platform == "win32", reason="uses POSIX signal delivery")
def test_interrupt_rolls_back_before_python_regains_control(
    cow_repository: Repository, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    git = shutil.which("git")
    assert git is not None
    tools = tmp_path / "tools"
    tools.mkdir()
    wrapper = tools / "git"
    wrapper.write_text(
        f"#!{sys.executable}\n"
        "import os, signal, subprocess, sys\n"
        f"result = subprocess.run([{git!r}, *sys.argv[1:]], check=False)\n"
        "if result.returncode == 0 and '--no-checkout' in sys.argv:\n"
        "    os.kill(os.getppid(), signal.SIGINT)\n"
        "raise SystemExit(result.returncode)\n"
    )
    wrapper.chmod(0o755)
    monkeypatch.setenv("PATH", str(tools) + os.pathsep + os.environ["PATH"])
    target = tmp_path / "interrupted"
    with pytest.raises(KeyboardInterrupt):
        add_worktree(request=cow_repository.add_request(target=target, branch="interrupted"))
    cow_repository.assert_absent(target=target, branch="interrupted")
    assert cow_repository.registered == {cow_repository.path}
