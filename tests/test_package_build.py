"""Packaging copies the newly built authority from its explicit Cargo output tree."""

from pathlib import Path

import pytest

from scripts import package


def test_package_pins_cargo_directories_and_copies_the_new_binary(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repository = tmp_path / "repository"
    release = repository / "target/release"
    release.mkdir(parents=True)
    (release / "cowtree-metadata").write_bytes(b"stale authority")
    (repository / "LICENSE").write_text("fixture license")
    (repository / "docs").mkdir()
    (repository / "docs/install.rst").write_text("fixture installation")
    foreign = tmp_path / "foreign"
    monkeypatch.setenv("CARGO_TARGET_DIR", str(foreign / "target"))
    monkeypatch.setenv("CARGO_BUILD_BUILD_DIR", str(foreign / "build"))
    monkeypatch.setattr(package, "__file__", str(repository / "scripts/package.py"))
    monkeypatch.setattr(package.sys, "platform", "darwin")
    monkeypatch.setattr(package.platform, "machine", lambda: "arm64")
    environments: list[dict[str, str]] = []

    def prepare(repository: Path, work: Path) -> Path:
        assert repository.is_dir()
        python = work / "venv/bin/python"
        return python

    def run(arguments: list[str], *, cwd: Path, env: dict[str, str], check: bool) -> None:
        assert check
        if arguments[0] == "cargo":
            environments.append(env)
            assert cwd == repository
            target = Path(env["CARGO_TARGET_DIR"]) / "release"
            target.mkdir(parents=True, exist_ok=True)
            (target / "cowtree-metadata").write_bytes(b"fresh authority")
        else:
            destination = Path(arguments[arguments.index("--distpath") + 1]) / "cowtree"
            destination.mkdir(parents=True)
            (destination / "cowtree").write_bytes(b"frozen CLI")

    monkeypatch.setattr(package, "prepare_python", prepare)
    monkeypatch.setattr(package.subprocess, "run", run)
    bundle = package.build(dist=tmp_path / "dist")
    assert len(environments) == 1
    assert environments[0]["CARGO_TARGET_DIR"] == str(repository / "target")
    assert environments[0]["CARGO_BUILD_BUILD_DIR"] == str(repository / "target")
    assert (bundle / "cowtree-metadata").read_bytes() == b"fresh authority"
    assert not foreign.exists()
