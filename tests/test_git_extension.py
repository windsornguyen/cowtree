"""Exercise agent commands through Git's installed external-command interface.

Git selects the repository and expands the local alias. The real git-cowtree
entry point must preserve arguments, private edits, and Cowtree's refusal rules.
No shell wrapper or in-process CLI substitute participates in these tests.
"""

import os
from pathlib import Path
import shutil
import sys
from typing import Literal

from pydantic import TypeAdapter
import pytest

from cowtree.cli_output import Failure, Success
from cowtree.errors import CowtreeErrorCode
from cowtree.types import Worktree
from tests.conftest import Repository


@pytest.fixture(autouse=True)
def installed_extension(monkeypatch: pytest.MonkeyPatch) -> None:
    """Select the native test binary or the installed Python entry point explicitly."""
    binary = os.environ.get("COWTREE_TEST_BINARY")
    binaries = str(Path(sys.executable if binary is None else binary).parent)
    assert shutil.which("git-cowtree", path=binaries) is not None
    monkeypatch.setenv("PATH", binaries + os.pathsep + os.environ["PATH"])


@pytest.mark.parametrize("command", ["cowtree", "wt"])
def test_git_entry_points_preserve_worktree_ownership(
    cow_repository: Repository, tmp_path: Path, command: Literal["cowtree", "wt"]
) -> None:
    repo = cow_repository
    repo.git("config", "--local", "alias.wt", "cowtree")
    (repo.path / "nested").mkdir()
    target = tmp_path / "agent tree"
    res = repo.git("-C", "nested", command, "add", "--json", "-b", "agent/task", "../../agent tree")
    created = TypeAdapter(Success).validate_json(res.stdout)
    assert isinstance(created.value, Worktree)
    assert created.value.path == target.resolve()
    assert created.value.branch == "refs/heads/agent/task"
    res = repo.git(command, "list", "--json")
    listed = TypeAdapter(list[Worktree]).validate_json(res.stdout)
    assert target.resolve() in {tree.path for tree in listed}
    (target / "file.txt").write_bytes(b"agent edit\n")
    assert (repo.path / "file.txt").read_bytes() == b"original\n"
    res = repo.git(command, "remove", "--json", str(target), check=False)
    assert res.returncode == 1
    assert TypeAdapter(Failure).validate_json(res.stderr).code is CowtreeErrorCode.COMMAND_FAILED
    assert (target / "file.txt").read_bytes() == b"agent edit\n"
    repo.git(command, "remove", "--force", str(target))
    assert target.resolve() not in repo.registered
    assert not target.exists()
    assert repo.git("rev-parse", "agent/task").stdout == repo.git("rev-parse", "HEAD").stdout


@pytest.mark.parametrize("command", ["cowtree", "wt"])
def test_git_entry_points_preserve_source_edits(
    cow_repository: Repository, tmp_path: Path, command: Literal["cowtree", "wt"]
) -> None:
    repo = cow_repository
    repo.git("config", "--local", "alias.wt", "cowtree")
    source = repo.path / "file.txt"
    source.write_bytes(b"staged edit\n")
    repo.git("add", "file.txt")
    source.write_bytes(b"unstaged edit\n")
    index = repo.git("diff", "--cached").stdout
    head = repo.git("rev-parse", "HEAD").stdout
    target = tmp_path / "committed tree"
    repo.git(command, "add", "--committed", "-b", "agent/task", str(target), "HEAD")
    assert (target / "file.txt").read_bytes() == b"original\n"
    assert source.read_bytes() == b"unstaged edit\n"
    assert repo.git("diff", "--cached").stdout == index
    assert repo.git("rev-parse", "HEAD").stdout == head
    repo.git(command, "remove", str(target))


@pytest.mark.parametrize("command", ["cowtree", "wt"])
def test_git_entry_points_refuse_unknown_flags_before_state_changes(
    repository: Repository, tmp_path: Path, command: Literal["cowtree", "wt"]
) -> None:
    repo = repository
    repo.git("config", "--local", "alias.wt", "cowtree")
    target = tmp_path / "rejected"
    res = repo.git(command, "add", "--json", "--force", "-b", "rejected", str(target), check=False)
    assert res.returncode == 2
    assert not res.stdout
    error = TypeAdapter(Failure).validate_json(res.stderr)
    assert error.code is CowtreeErrorCode.INVALID_ARGUMENTS
    repo.assert_absent(target=target, branch="rejected")


def test_alias_cannot_replace_the_builtin_worktree_command(repository: Repository) -> None:
    repo = repository
    repo.git("config", "--local", "alias.worktree", "cowtree")
    res = repo.git("worktree", "list", "--porcelain")
    assert res.stdout.startswith(f"worktree {repo.path.as_posix()}\n")
    res = repo.git("cowtree", "list", "--porcelain", check=False)
    assert res.returncode == 2
