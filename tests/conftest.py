from __future__ import annotations

from dataclasses import dataclass, field
import os
from pathlib import Path

import pytest

from cowtree.core import inspect_path, list_all_worktrees
from cowtree.exec import CommandRunner
from cowtree.types import CommandResult, WorktreeAddRequest


@dataclass(frozen=True)
class Repository:
    path: Path
    runner: CommandRunner = field(default_factory=CommandRunner)

    def git(self, *args: str, check: bool = True) -> CommandResult:
        result = self.runner.run(["git", "-C", str(self.path), *args], check=check)
        return result

    def commit(self, message: str = "fixture") -> str:
        self.git("add", "--all")
        self.git("commit", "-qm", message)
        commit = self.git("rev-parse", "HEAD").stdout.strip()
        return commit

    def add_request(
        self,
        target: Path,
        branch: str | None = None,
        *,
        commitish: str = "HEAD",
        lock: bool = False,
    ) -> WorktreeAddRequest:
        result = WorktreeAddRequest(
            path=target, source=self.path, branch=branch, commitish=commitish, lock=lock
        )
        return result

    @property
    def registered(self) -> set[Path]:
        paths = {worktree.path for worktree in list_all_worktrees(source=self.path)}
        return paths

    def assert_absent(self, target: Path, branch: str) -> None:
        assert not target.exists()
        assert target.resolve() not in self.registered
        assert self.git("show-ref", "--verify", f"refs/heads/{branch}", check=False).returncode != 0

    def assert_clean(self, target: Path, commit: str | None = None) -> None:
        if commit is not None:
            assert self.git("-C", str(target), "rev-parse", "HEAD").stdout.strip() == commit
        status = self.git("-C", str(target), "status", "--porcelain=v1", "--untracked-files=all")
        assert status.stdout == ""
        assert self.git("-C", str(target), "diff", "HEAD", "--exit-code").returncode == 0


@pytest.fixture
def repository(tmp_path: Path) -> Repository:
    path = tmp_path / "repo"
    path.mkdir()
    repo = Repository(path=path.resolve())
    repo.git("init", "-q")
    repo.git("config", "user.email", "test@example.com")
    repo.git("config", "user.name", "Cowtree test")
    repo.git("config", "commit.gpgsign", "false")
    repo.git("config", "core.autocrlf", "false")
    (path / "file.txt").write_bytes(b"original\n")
    repo.commit()
    return repo


@pytest.fixture
def cow_repository(repository: Repository) -> Repository:
    report = inspect_path(path=repository.path)
    if os.environ.get("COWTREE_EXPECT_SUPPORTED") == "1":
        assert report.supported, report.reason
    if not report.supported:
        assert report.reason is not None
        pytest.skip(report.reason)
    return repository
