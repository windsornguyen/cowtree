"""Materialize a pinned commit without changing the caller's checkout or index.

The caller holds the repository lock. A private Git checkout applies Git's file
conversions once; the destination still uses native CoW. The seed is removed
before returning, so this mode retains no duplicate checked-out source payload.
"""

from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
import shutil
import tempfile

from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.fs import doctor
from cowtree.git import GitRepository, parse_tracked_files
from cowtree.types import WorktreeAddRequest


@dataclass(frozen=True)
class CommittedSource:
    """Own a temporary seed for one immutable commit and its Git registration."""

    repository: GitRepository
    commit: str

    @classmethod
    def resolve(cls, repository: GitRepository, request: WorktreeAddRequest) -> "CommittedSource":
        reference = request.commitish
        if request.existing_branch is not None:
            if reference != "HEAD":
                raise CowtreeError(
                    CowtreeErrorCode.INVALID_ARGUMENTS,
                    "--branch selects the commit; omit commit-ish",
                )
            reference = f"refs/heads/{request.existing_branch}"
        commit = repository.capture(
            args=["rev-parse", "--verify", "--end-of-options", f"{reference}^{{commit}}"]
        ).removesuffix("\n")
        parse_tracked_files(data=repository.capture(args=["ls-tree", "-r", "-z", commit]))
        result = cls(repository=repository, commit=commit)
        return result

    @contextmanager
    def open(self) -> Iterator[GitRepository]:
        """Remove only the private seed; retain evidence if Git cleanup fails."""
        parent = self.repository.path.parent
        report = doctor(path=parent)
        if not report.supported:
            raise CowtreeError(
                CowtreeErrorCode.COW_UNAVAILABLE, f"CoW unavailable: {report.reason}"
            )
        root = Path(tempfile.mkdtemp(prefix=".cowtree-seed-", dir=parent))
        path = root / "tree"
        try:
            self.repository.run(
                args=["worktree", "add", "--detach", "--lock", "--", str(path), self.commit]
            )
            yield GitRepository(io=self.repository.io, path=path)
        finally:
            self.remove(root=root, path=path)

    def remove(self, root: Path, path: Path) -> None:
        """Retire the owned registration before deleting its private parent directory."""
        try:
            if self.repository.find_worktree(path=path) is not None:
                self.repository.run(args=["worktree", "remove", "--force", "--force", str(path)])
            shutil.rmtree(root)
        except (CowtreeError, OSError) as error:
            raise CowtreeError(
                CowtreeErrorCode.CLEANUP_FAILED,
                f"seed cleanup failed at {root}; destination may be complete; "
                "inspect git worktree list before retrying",
            ) from error
