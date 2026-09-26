"""Explicit submodule admission and the Git identities retained by a managed capture."""

from dataclasses import dataclass
from enum import Enum


class SubmodulePolicy(str, Enum):
    """How a capture treats the gitlinks its source tree declares."""

    REJECT = "reject"
    # A gitlink the source never initialized stays an empty directory, as `git worktree add`
    # leaves it. An initialized one is refused, because this policy copies no submodule state.
    LEAVE_UNINITIALIZED = "leave-uninitialized"
    MATERIALIZE_PINNED = "materialize-pinned"


@dataclass(frozen=True)
class PinnedSubmodule:
    path: str
    commit: str
