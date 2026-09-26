from __future__ import annotations

from dataclasses import replace
import errno
import os
from pathlib import Path
import sys
from typing import NoReturn

import pytest

from cowtree.core import add_worktree, inspect_path, remove_worktree
from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.exec import CommandRunner
from cowtree.submodule_types import SubmodulePolicy
from cowtree.types import SourceMode, WorktreeAddRequest

from .conftest import Repository


def test_invariant_filesystem_support_matches_expectation(tmp_path: Path) -> None:
    report = inspect_path(path=tmp_path)
    expected = os.environ.get("COWTREE_EXPECT_SUPPORTED")
    if expected is not None:
        assert expected in ("0", "1")
        assert report.supported == (expected == "1"), report


def test_invariant_unsupported_filesystem_leaves_no_state(
    repository: Repository, tmp_path: Path
) -> None:
    if inspect_path(path=repository.path).supported:
        pytest.skip("requires a filesystem without CoW support")
    target = tmp_path / "unsupported"
    with pytest.raises(CowtreeError) as error:
        add_worktree(request=repository.add_request(target=target, branch="unsupported"))
    assert error.value.code == CowtreeErrorCode.COW_UNAVAILABLE
    repository.assert_absent(target=target, branch="unsupported")


@pytest.mark.parametrize(
    "name",
    [
        "with space",
        "tab\tname",
        "line\nname",
        "carriage\rname",
        "-leading",
        "--",
        "quote'\"name",
        "back\\slash",
        "café",
        "cafe\u0301",
        "🐄",
        ".hidden",
        "x" * 200,
    ],
)
def test_invariant_tracked_names_and_worktree_paths_round_trip(
    cow_repository: Repository, tmp_path: Path, name: str
) -> None:
    repo = cow_repository
    filename = f"{name}.txt"
    (repo.path / filename).write_bytes(b"a\r\nb\nc\x00\xff")
    repo.commit()
    target = tmp_path / name
    worktree = add_worktree(request=repo.add_request(target=target, branch="names"))
    assert worktree.path.samefile(target)
    assert (target / filename).read_bytes() == (repo.path / filename).read_bytes()
    assert any(path.samefile(target) for path in repo.registered)
    repo.assert_clean(target=target)
    remove_worktree(path=target, source=repo.path)
    assert not target.exists()


def test_invariant_non_utf8_names_round_trip(cow_repository: Repository, tmp_path: Path) -> None:
    repo = cow_repository
    filename = os.fsdecode(b"raw-\xff-\xfe")
    try:
        (repo.path / filename).write_bytes(b"raw bytes\n")
    except OSError as error:
        if error.errno == errno.EILSEQ or (sys.platform == "darwin" and error.errno == errno.EPERM):
            pytest.skip("filesystem rejects non-UTF-8 filenames")
        raise
    repo.commit()
    target = tmp_path / f"target-{filename}"
    add_worktree(request=repo.add_request(target=target, branch="raw-bytes"))
    assert (target / filename).read_bytes() == b"raw bytes\n"
    assert target.resolve() in repo.registered
    repo.assert_clean(target=target)
    remove_worktree(path=target, source=repo.path)


def test_invariant_checkout_preserves_modes_symlinks_and_isolation(
    cow_repository: Repository, tmp_path: Path
) -> None:
    repo = cow_repository
    (repo.path / "run").write_bytes(b"#!/bin/sh\nexit 0\n")
    (repo.path / "run").chmod(0o755)
    (repo.path / "file-link").symlink_to("file.txt")
    (repo.path / "dangling").symlink_to("missing")
    (repo.path / "directory-link").symlink_to("nested")
    (repo.path / "nested").mkdir()
    (repo.path / "nested" / "data").write_bytes(b"nested\n")
    repo.commit()
    target = tmp_path / "target"
    add_worktree(request=repo.add_request(target=target, branch="isolation"))
    assert (target / "run").stat().st_mode & 0o777 == 0o755
    for filename in ("file-link", "dangling", "directory-link"):
        assert os.readlink(target / filename) == os.readlink(repo.path / filename)
    repo.assert_clean(target=target)
    (target / "file.txt").write_bytes(b"target change\n")
    assert (repo.path / "file.txt").read_bytes() == b"original\n"
    (repo.path / "nested" / "data").write_bytes(b"source change\n")
    assert (target / "nested" / "data").read_bytes() == b"nested\n"


def test_invariant_clones_can_be_sources(cow_repository: Repository, tmp_path: Path) -> None:
    repo = cow_repository
    first = tmp_path / "first"
    second = tmp_path / "second"
    add_worktree(request=repo.add_request(target=first, branch="first"))
    add_worktree(request=WorktreeAddRequest(path=second, source=first, branch="second"))
    repo.assert_clean(target=second)
    assert (second / "file.txt").read_bytes() == b"original\n"
    remove_worktree(path=first, source=repo.path)
    repo.assert_clean(target=second)
    remove_worktree(path=second, source=repo.path)
    assert repo.registered == {repo.path}


def test_invariant_relative_paths_use_callers_directory(
    cow_repository: Repository, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = cow_repository
    nested = tmp_path / "caller" / "nested"
    nested.mkdir(parents=True)
    monkeypatch.chdir(nested)
    worktree = add_worktree(request=repo.add_request(target=Path("../target"), branch="relative"))
    assert worktree.path == (nested.parent / "target").resolve()
    remove_worktree(path=Path("../target"), source=repo.path)
    assert repo.registered == {repo.path}


@pytest.mark.parametrize(
    "state", ["modified", "staged", "deleted", "assume-unchanged", "skip-worktree"]
)
def test_invariant_tracked_source_changes_are_refused(
    repository: Repository, tmp_path: Path, state: str
) -> None:
    repo = repository
    path = repo.path / "file.txt"
    if state in ("assume-unchanged", "skip-worktree"):
        repo.git("update-index", f"--{state}", "--", "file.txt")
    if state == "deleted":
        path.unlink()
    else:
        path.write_bytes(b"changed\n")
    if state == "staged":
        repo.git("add", "file.txt")
    target = tmp_path / "refused"
    with pytest.raises(CowtreeError):
        add_worktree(request=repo.add_request(target=target, branch="refused"))
    repo.assert_absent(target=target, branch="refused")


def test_invariant_untracked_source_files_are_not_cloned(
    cow_repository: Repository, tmp_path: Path
) -> None:
    repo = cow_repository
    (repo.path / "untracked").write_bytes(b"private working data\n")
    target = tmp_path / "target"
    add_worktree(request=repo.add_request(target=target, branch="tracked-only"))
    assert not (target / "untracked").exists()
    repo.assert_clean(target=target)


def test_invariant_sparse_sources_are_refused(repository: Repository, tmp_path: Path) -> None:
    repo = repository
    repo.git("sparse-checkout", "init", "--cone")
    target = tmp_path / "sparse"
    with pytest.raises(CowtreeError) as error:
        add_worktree(request=repo.add_request(target=target, branch="sparse"))
    assert error.value.code == CowtreeErrorCode.SPARSE_CHECKOUT
    repo.assert_absent(target=target, branch="sparse")


def declare_gitlink(repo: Repository) -> str:
    """Commit a gitlink at `module` that the source never initializes, as a fresh clone has it."""
    commit = repo.git("rev-parse", "HEAD").stdout.strip()
    (repo.path / ".gitmodules").write_bytes(
        b'[submodule "module"]\n\tpath = module\n\turl = ./module\n'
    )
    repo.git("add", ".gitmodules")
    repo.git("update-index", "--add", "--cacheinfo", f"160000,{commit},module")
    repo.git("commit", "-qm", "gitlink")
    (repo.path / "module").mkdir()
    return commit


def test_invariant_submodule_sources_are_refused_under_reject(
    repository: Repository, tmp_path: Path
) -> None:
    repo = repository
    declare_gitlink(repo)
    target = tmp_path / "submodule"
    request = replace(
        repo.add_request(target=target, branch="submodule"), submodules=SubmodulePolicy.REJECT
    )
    with pytest.raises(CowtreeError) as error:
        add_worktree(request=request)
    assert error.value.code == CowtreeErrorCode.SUBMODULE_UNSUPPORTED
    repo.assert_absent(target=target, branch="submodule")


def test_invariant_uninitialized_submodules_stay_empty_by_default(
    repository: Repository, tmp_path: Path
) -> None:
    """An uninitialized gitlink clones as an empty directory whose index keeps the commit."""
    repo = repository
    commit = declare_gitlink(repo)
    for mode in (SourceMode.CHECKOUT, SourceMode.COMMIT):
        target = tmp_path / mode.value
        request = replace(repo.add_request(target=target), detach=True, source_mode=mode)
        add_worktree(request=request)
        assert (target / "module").is_dir()
        assert not any((target / "module").iterdir())
        status = repo.git("-C", str(target), "submodule", "status").stdout
        assert status == f"-{commit} module\n"
        assert repo.git("-C", str(target), "status", "--porcelain").stdout == ""


def test_invariant_initialized_submodules_are_refused_by_default(
    repository: Repository, tmp_path: Path
) -> None:
    """The default policy copies no submodule state, so a checked-out one is refused."""
    repo = repository
    declare_gitlink(repo)
    (repo.path / "module" / "file.txt").write_bytes(b"checked out\n")
    target = tmp_path / "initialized"
    with pytest.raises(CowtreeError) as error:
        add_worktree(request=repo.add_request(target=target, branch="initialized"))
    assert error.value.code == CowtreeErrorCode.SUBMODULE_INITIALIZED
    assert "module" in error.value.message
    repo.assert_absent(target=target, branch="initialized")


def test_invariant_failed_revision_does_not_leave_a_branch(
    repository: Repository, tmp_path: Path
) -> None:
    repo = repository
    (repo.path / "file.txt").write_bytes(b"second\n")
    repo.commit()
    target = tmp_path / "old"
    with pytest.raises(CowtreeError) as error:
        add_worktree(request=repo.add_request(target=target, branch="failed", commitish="HEAD~1"))
    assert error.value.code == CowtreeErrorCode.HEAD_MISMATCH
    repo.assert_absent(target=target, branch="failed")


def test_invariant_existing_targets_are_preserved(repository: Repository, tmp_path: Path) -> None:
    repo = repository
    target = tmp_path / "existing"
    target.mkdir()
    sentinel = target / "sentinel"
    sentinel.write_bytes(b"owned by caller\n")
    with pytest.raises(CowtreeError):
        add_worktree(request=repo.add_request(target=target, branch="existing"))
    assert sentinel.read_bytes() == b"owned by caller\n"
    assert target.resolve() not in repo.registered
    assert repo.git("branch", "--list", "existing").stdout == ""


@pytest.mark.parametrize("state", ["modified", "untracked", "locked"])
def test_invariant_removal_preserves_work_that_requires_force(
    cow_repository: Repository, tmp_path: Path, state: str
) -> None:
    repo = cow_repository
    target = tmp_path / "protected"
    add_worktree(
        request=repo.add_request(target=target, branch="protected", lock=state == "locked")
    )
    if state == "modified":
        (target / "file.txt").write_bytes(b"keep my edits\n")
    if state == "untracked":
        (target / "untracked").write_bytes(b"keep my work\n")
    with pytest.raises(CowtreeError):
        remove_worktree(path=target, source=repo.path)
    assert target.exists()
    assert target.resolve() in repo.registered
    if state == "locked":
        with pytest.raises(CowtreeError):
            remove_worktree(path=target, source=repo.path, force=True)
        repo.git("worktree", "unlock", str(target))
    remove_worktree(path=target, source=repo.path, force=True)
    assert not target.exists()
    assert repo.git("show-ref", "--verify", "refs/heads/protected").returncode == 0


def test_invariant_large_checkout_has_exact_payload(
    cow_repository: Repository, tmp_path: Path
) -> None:
    repo = cow_repository
    payload = bytes(range(256)) * (4 * 1024)
    for index in range(256):
        folder = repo.path / f"directory-{index % 8}"
        folder.mkdir(exist_ok=True)
        (folder / f"file-{index}").write_bytes(str(index).encode() + b"\n")
    (repo.path / "large").write_bytes(payload * 8)
    repo.commit()
    target = tmp_path / "large"
    add_worktree(request=repo.add_request(target=target, branch="large"))
    assert (target / "large").read_bytes() == payload * 8
    for index in range(256):
        assert (target / f"directory-{index % 8}" / f"file-{index}").read_bytes() == str(
            index
        ).encode() + b"\n"
    repo.assert_clean(target=target)


def test_invariant_default_add_is_detached(cow_repository: Repository, tmp_path: Path) -> None:
    repo = cow_repository
    worktree = add_worktree(request=repo.add_request(target=tmp_path / "detached"))
    assert worktree.detached
    assert worktree.branch is None
    repo.assert_clean(target=worktree.path)


def test_invariant_existing_branches_are_preserved(repository: Repository, tmp_path: Path) -> None:
    repo = repository
    repo.git("branch", "existing")
    original = repo.git("rev-parse", "existing").stdout
    target = tmp_path / "existing"
    with pytest.raises(CowtreeError):
        add_worktree(request=repo.add_request(target=target, branch="existing"))
    assert not target.exists()
    assert repo.registered == {repo.path}
    assert repo.git("rev-parse", "existing").stdout == original


@pytest.mark.parametrize("operation", ["add", "remove"])
def test_invariant_unresolvable_paths_raise_typed_errors(
    repository: Repository, tmp_path: Path, operation: str
) -> None:
    loop = tmp_path / "loop"
    loop.symlink_to("loop")
    if operation == "add":
        with pytest.raises(CowtreeError):
            add_worktree(request=repository.add_request(target=loop / "target", branch="loop"))
    else:
        with pytest.raises(CowtreeError):
            remove_worktree(path=loop / "target", source=repository.path)
    assert repository.registered == {repository.path}
    assert loop.is_symlink()


def test_cross_filesystem_add_fails_before_probing_or_creating_state(
    repository: Repository, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = repository
    target = tmp_path / "new-parent" / "cross-filesystem"
    original = Path.stat

    def other_device(self: Path, *, follow_symlinks: bool = True) -> os.stat_result:
        result = original(self, follow_symlinks=follow_symlinks)
        if self == repo.path:
            result = os.stat_result(
                (
                    result.st_mode,
                    result.st_ino,
                    result.st_dev + 1,
                    result.st_nlink,
                    result.st_uid,
                    result.st_gid,
                    result.st_size,
                    result.st_atime,
                    result.st_mtime,
                    result.st_ctime,
                )
            )
        return result

    def unexpected_probe(path: Path, runner: CommandRunner | None = None) -> NoReturn:
        del path, runner
        raise AssertionError("cross-filesystem add must fail before the clone probe")

    monkeypatch.setattr(Path, "stat", other_device)
    monkeypatch.setattr("cowtree.core.doctor", unexpected_probe)
    with pytest.raises(CowtreeError) as error:
        add_worktree(request=repo.add_request(target=target, branch="cross-filesystem"))
    assert error.value.code == CowtreeErrorCode.DIFFERENT_FILESYSTEM
    repo.assert_absent(target=target, branch="cross-filesystem")
    assert not target.parent.exists()
