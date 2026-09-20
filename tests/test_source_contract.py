from __future__ import annotations

import os
from pathlib import Path
from typing import Literal

import pytest

from cowtree.core import add_worktree, list_all_worktrees, remove_worktree
from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.types import WorktreeAddRequest

from .conftest import Repository


@pytest.mark.parametrize("conversion", ["crlf", "ident", "utf16"])
def test_checkout_conversions_preserve_clean_working_bytes(
    cow_repository: Repository, tmp_path: Path, conversion: Literal["crlf", "ident", "utf16"]
) -> None:
    repo = cow_repository
    source = repo.path / "converted.txt"
    if conversion == "crlf":
        attributes = "converted.txt text eol=crlf\n"
        working = b"one\r\ntwo\r\n"
    elif conversion == "ident":
        attributes = "converted.txt ident\n"
        working = b"version $Id$\n"
    else:
        attributes = "converted.txt text working-tree-encoding=UTF-16\n"
        working = "hello λ\n".encode("utf-16")
    (repo.path / ".gitattributes").write_text(attributes)
    source.write_bytes(working)
    commit = repo.commit()
    source.unlink()
    repo.git("checkout", "--", source.name)
    working = source.read_bytes()
    if conversion == "crlf":
        assert working == b"one\r\ntwo\r\n"
    elif conversion == "ident":
        assert working.startswith(b"version $Id: ")
    else:
        assert working.decode("utf-16") == "hello λ\n"
    assert repo.git("status", "--porcelain=v1").stdout == ""
    index_entries = repo.git("ls-files", "--stage", "-z").stdout
    target = tmp_path / conversion
    add_worktree(request=WorktreeAddRequest(source=repo.path, path=target, branch=conversion))
    assert (target / source.name).read_bytes() == working
    assert repo.git("ls-files", "--stage", "-z").stdout == index_entries
    repo.assert_clean(target=target, commit=commit)
    remove_worktree(path=target, source=repo.path)


def test_shared_autocrlf_configuration_preserves_crlf(
    cow_repository: Repository, tmp_path: Path
) -> None:
    repo = cow_repository
    repo.git("config", "core.autocrlf", "true")
    source = repo.path / "file.txt"
    source.unlink()
    repo.git("checkout", "--", source.name)
    assert source.read_bytes() == b"original\r\n"
    commit = repo.git("rev-parse", "HEAD").stdout.strip()
    target = tmp_path / "autocrlf"
    add_worktree(request=WorktreeAddRequest(source=repo.path, path=target))
    assert (target / source.name).read_bytes() == source.read_bytes()
    repo.assert_clean(target=target, commit=commit)


@pytest.mark.parametrize("mode", [0o700, 0o751, 0o755])
def test_committed_executable_modes_are_preserved(
    cow_repository: Repository, tmp_path: Path, mode: int
) -> None:
    repo = cow_repository
    source = repo.path / "run.sh"
    source.write_bytes(b"#!/bin/sh\nexit 0\n")
    source.chmod(mode)
    commit = repo.commit()
    assert repo.git("ls-tree", "HEAD", "--", source.name).stdout.startswith("100755 blob ")
    repo.git("config", "core.filemode", "false")
    target = tmp_path / "executable"
    add_worktree(request=WorktreeAddRequest(source=repo.path, path=target))
    assert (target / source.name).stat().st_mode == source.stat().st_mode
    assert (target / source.name).read_bytes() == source.read_bytes()
    repo.assert_clean(target=target, commit=commit)


@pytest.mark.parametrize("change_content", [False, True])
def test_target_content_check_survives_unchanged_source_stat_cache(
    cow_repository: Repository, tmp_path: Path, change_content: bool
) -> None:
    repo = cow_repository
    source = repo.path / "file.txt"
    repo.git("config", "core.trustctime", "false")
    repo.git("config", "core.checkStat", "minimal")
    timestamp = 1_700_000_000_000_000_000
    os.utime(source, ns=(timestamp, timestamp))
    repo.git("update-index", "--refresh")
    source.write_bytes(b"modified\n" if change_content else b"original\n")
    os.utime(source, ns=(timestamp, timestamp))
    assert repo.git("status", "--porcelain=v1").stdout == ""
    target = tmp_path / "stat-cache"
    request = WorktreeAddRequest(source=repo.path, path=target, branch="stat-cache")
    if change_content:
        with pytest.raises(CowtreeError) as caught:
            add_worktree(request=request)
        assert caught.value.code == CowtreeErrorCode.DIRTY_SOURCE
        assert not target.exists()
        assert not any(tree.path == target for tree in list_all_worktrees(source=repo.path))
        assert (
            repo.git("show-ref", "--verify", "refs/heads/stat-cache", check=False).returncode != 0
        )
    else:
        commit = repo.git("rev-parse", "HEAD").stdout.strip()
        add_worktree(request=request)
        repo.assert_clean(target=target, commit=commit)


def test_worktree_autocrlf_configuration_preserves_clean_bytes(
    cow_repository: Repository, tmp_path: Path
) -> None:
    repo = cow_repository
    repo.git("config", "extensions.worktreeConfig", "true")
    repo.git("config", "--worktree", "core.autocrlf", "true")
    source = repo.path / "file.txt"
    source.unlink()
    repo.git("checkout", "--", source.name)
    assert source.read_bytes() == b"original\r\n"
    commit = repo.git("rev-parse", "HEAD").stdout.strip()
    target = tmp_path / "worktree-config"
    add_worktree(request=WorktreeAddRequest(source=repo.path, path=target))
    assert (target / source.name).read_bytes() == source.read_bytes()
    assert repo.git("-C", str(target), "config", "--bool", "core.autocrlf").stdout == "true\n"
    repo.assert_clean(target=target, commit=commit)


def test_source_worktree_location_setting_does_not_redirect_target(
    cow_repository: Repository, tmp_path: Path
) -> None:
    repo = cow_repository
    repo.git("config", "core.worktree", str(repo.path))
    target = tmp_path / "worktree-location"
    add_worktree(request=WorktreeAddRequest(source=repo.path, path=target))
    assert (
        Path(repo.git("-C", str(target), "rev-parse", "--show-toplevel").stdout.removesuffix("\n"))
        == target
    )
    (target / "file.txt").write_bytes(b"target changed\n")
    assert repo.git("-C", str(target), "status", "--porcelain=v1").stdout == " M file.txt\n"
    assert (repo.path / "file.txt").read_bytes() == b"original\n"
    assert repo.git("status", "--porcelain=v1").stdout == ""
