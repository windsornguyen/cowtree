"""The offline checker never accepts a different vendored or cached executable."""

import hashlib
from pathlib import Path

import pytest

from scripts import check_specs


def test_checker_prepares_the_pinned_artifact_without_network(tmp_path: Path) -> None:
    checker = check_specs.Checker(cache=tmp_path, java="unused", timeout_seconds=1)
    checker.prepare()
    assert hashlib.sha256(checker.jar.read_bytes()).hexdigest() == check_specs.TLC_SHA256
    assert checker.jar.read_bytes() == check_specs.TLC_JAR.read_bytes()
    checker.prepare()


def test_checker_rejects_changed_cache(tmp_path: Path) -> None:
    checker = check_specs.Checker(cache=tmp_path, java="unused", timeout_seconds=1)
    checker.jar.write_bytes(b"different checker")
    with pytest.raises(check_specs.SpecCheckError, match=r"cached.*SHA-256"):
        checker.prepare()
    assert checker.jar.read_bytes() == b"different checker"


def test_checker_rejects_changed_vendor_even_with_valid_cache(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    checker = check_specs.Checker(cache=tmp_path, java="unused", timeout_seconds=1)
    checker.prepare()
    source = tmp_path / "different.jar"
    source.write_bytes(b"different checker")
    monkeypatch.setattr(check_specs, "TLC_JAR", source)
    with pytest.raises(check_specs.SpecCheckError, match=r"vendored.*SHA-256"):
        checker.prepare()
