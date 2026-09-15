from __future__ import annotations

from enum import Enum


class CowtreeErrorCode(str, Enum):
    DIRTY_SOURCE = "dirty_source"
    DIFFERENT_FILESYSTEM = "different_filesystem"
    COW_UNAVAILABLE = "cow_unavailable"
    HEAD_MISMATCH = "head_mismatch"
    SPARSE_CHECKOUT = "sparse_checkout"
    SUBMODULE_UNSUPPORTED = "submodule_unsupported"
    UNSUPPORTED_MODE = "unsupported_mode"
    COMMAND_FAILED = "command_failed"
    INVALID_ARGUMENTS = "invalid_arguments"
    WORKTREE_NOT_FOUND = "worktree_not_found"
    CLEANUP_FAILED = "cleanup_failed"


class CowtreeError(RuntimeError):
    def __init__(self, code: CowtreeErrorCode, message: str) -> None:
        super().__init__(message)
        self.code = code
        self.message = message
