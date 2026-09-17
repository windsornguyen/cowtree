"""Project selected source paths into Git without changing a caller's index."""

from __future__ import annotations

from dataclasses import dataclass
import os
from pathlib import Path
import re
import subprocess
import tempfile

from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.git import GitRepository
from cowtree.install import local_path


@dataclass(frozen=True)
class ProjectionSpec:
    parent: str | None = None
    reuse: str | None = None


@dataclass(frozen=True)
class GitProjection:
    """Own source-only Git objects and refs under the cowtree namespace."""

    repository: GitRepository

    def create(
        self,
        tree: Path,
        paths: tuple[str, ...],
        parents: tuple[str, ...] = (),
        reuse: str | None = None,
    ) -> str:
        """Build a source commit with an isolated index and candidate attributes."""
        for path in paths:
            local_path(root=tree, relative=path)
        git_directory = self.repository.capture(
            args=["rev-parse", "--path-format=absolute", "--git-common-dir"]
        ).removesuffix("\n")
        with tempfile.TemporaryDirectory(prefix=".cowtree-index-", dir=tree.parent) as scratch:
            environment = os.environ.copy()
            environment["GIT_INDEX_FILE"] = str(Path(scratch) / "index")
            command = [
                "git",
                "--literal-pathspecs",
                "--git-dir",
                git_directory,
                "--work-tree",
                str(tree),
                "-c",
                "core.filemode=true",
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.sparseCheckout=false",
            ]
            self.execute(
                command=[*command, "read-tree", "--empty"], directory=tree, environment=environment
            )
            if paths:
                data = b"".join(os.fsencode(path) + b"\0" for path in paths)
                self.execute(
                    command=[
                        *command,
                        "add",
                        "--force",
                        "--pathspec-from-file=-",
                        "--pathspec-file-nul",
                    ],
                    directory=tree,
                    environment=environment,
                    data=data,
                )
            tree_id = self.execute(
                command=[*command, "write-tree"], directory=tree, environment=environment
            ).strip()
            if reuse is not None:
                self.validate_commit(commit=reuse, tree=tree_id, parents=parents)
                return reuse
            arguments = [*command, "commit-tree", tree_id]
            for parent in parents:
                if not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", parent):
                    raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "invalid Git parent")
                arguments.extend(["-p", parent])
            commit = self.execute(
                command=[*arguments, "-m", "cowtree snapshot"],
                directory=tree,
                environment=environment,
            ).strip()
        if not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", commit):
            raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, "Git returned an invalid commit")
        return commit

    def validate_commit(self, commit: str, tree: str, parents: tuple[str, ...]) -> None:
        """Reuse a checked commit only when its source tree and ordered parents match."""
        if not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", commit):
            raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "invalid validated Git identity")
        identity = self.repository.capture(
            args=["rev-parse", "--verify", "--end-of-options", f"{commit}^{{tree}}"]
        )
        ancestry = self.repository.capture(
            args=["rev-list", "--parents", "-n", "1", commit]
        ).split()
        if identity.strip() != tree or tuple(ancestry[1:]) != parents:
            raise CowtreeError(
                CowtreeErrorCode.COMMAND_FAILED, "projection differs from validated commit"
            )

    def set_ref(self, reference: str, commit: str, expected: str | None = None) -> None:
        """Publish only within refs/cowtree, with an explicit previous-value check."""
        if not reference.startswith("refs/cowtree/"):
            raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "ref is outside refs/cowtree")
        if any(
            not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", value)
            for value in (commit, expected)
            if value is not None
        ):
            raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "invalid Git ref value")
        self.repository.run(args=["check-ref-format", reference])
        current = self.repository.run(
            args=["rev-parse", "--verify", "--quiet", "--end-of-options", reference], check=False
        )
        if current.returncode == 0 and current.stdout.removesuffix("\n") == commit:
            return
        if current.returncode not in (0, 1):
            raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, current.stderr)
        previous = "0" * len(commit) if expected is None else expected
        self.repository.run(args=["update-ref", reference, commit, previous])

    def remove_ref(self, reference: str, expected: str) -> None:
        """Remove an owned ref only if it still names the expected commit."""
        if not reference.startswith("refs/cowtree/") or not re.fullmatch(
            r"[0-9a-f]{40}|[0-9a-f]{64}", expected
        ):
            raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "invalid owned Git reference")
        self.repository.run(args=["check-ref-format", reference])
        current = self.repository.run(
            args=["rev-parse", "--verify", "--quiet", "--end-of-options", reference], check=False
        )
        if current.returncode == 1:
            return
        if current.returncode != 0:
            raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, current.stderr)
        self.repository.run(args=["update-ref", "-d", reference, expected])

    def execute(
        self,
        command: list[str],
        directory: Path,
        environment: dict[str, str],
        data: bytes | None = None,
    ) -> str:
        result = subprocess.run(  # noqa: S603
            command,
            cwd=directory,
            env=environment,
            input=data,
            capture_output=True,
            check=False,
            pass_fds=self.repository.io.descriptors,
        )
        if result.returncode != 0:
            raise CowtreeError(
                CowtreeErrorCode.COMMAND_FAILED, result.stderr.decode(errors="replace")
            )
        output = result.stdout.decode("utf-8")
        return output
