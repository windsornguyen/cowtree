from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path

import pytest

from cowtree.exec import CommandRunner
from cowtree.models import CommandResult


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


@pytest.fixture
def repository(tmp_path: Path) -> Repository:
    path = tmp_path / "repo"
    path.mkdir()
    repo = Repository(path.resolve())
    repo.git("init", "-q")
    repo.git("config", "user.email", "test@example.com")
    repo.git("config", "user.name", "Cowtree test")
    repo.git("config", "commit.gpgsign", "false")
    repo.git("config", "core.autocrlf", "false")
    (path / "file.txt").write_bytes(b"original\n")
    repo.commit()
    return repo
