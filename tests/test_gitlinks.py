"""Gitlinks remain explicit records and are never silently treated as absent files."""

import pytest

from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.git import parse_tracked_files, parse_tree
from cowtree.submodule_types import PinnedSubmodule, SubmodulePolicy
from cowtree.types import FileMode, TrackedFile


def test_gitlinks_require_explicit_policy_and_retain_their_commit() -> None:
    commit = "a" * 40
    data = f"100644 blob {'b' * 40}\tmain.rs\x00160000 commit {commit}\tvendor/dep\x00"
    with pytest.raises(CowtreeError) as refused:
        parse_tracked_files(data)
    assert refused.value.code is CowtreeErrorCode.SUBMODULE_UNSUPPORTED
    tree = parse_tree(data, SubmodulePolicy.MATERIALIZE_PINNED)
    assert tree.files == (TrackedFile(mode=FileMode.REGULAR, path="main.rs"),)
    assert tree.submodules == (PinnedSubmodule(path="vendor/dep", commit=commit),)
