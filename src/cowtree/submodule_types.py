"""Explicit submodule admission and the Git identities retained by a managed capture."""

from dataclasses import dataclass
from enum import Enum


class SubmodulePolicy(str, Enum):
    REJECT = "reject"
    MATERIALIZE_PINNED = "materialize-pinned"


@dataclass(frozen=True)
class PinnedSubmodule:
    path: str
    commit: str
