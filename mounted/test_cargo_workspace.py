"""Real Cargo outputs remain private; unrepresentable hard links fail before import."""

import os
from pathlib import Path
import shutil

import pytest

from benchmarks.cargo_workspace import (
    BuildSpec,
    Config,
    Mutation,
    build,
    inventory,
    run,
    verify,
)
from cowtree.errors import CowtreeError
from cowtree.exec import CommandRunner
from cowtree.tree_types import DerivedHardlinks, PathPolicy
from cowtree.workspace import Workspace

from .test_bootstrap import metadata_binary


def test_oracle_detects_permission_only_changes(tmp_path: Path) -> None:
    file = tmp_path / "cache-artifact"
    file.write_bytes(b"unchanged bytes")
    file.chmod(0o644)
    before = inventory(root=tmp_path)
    file.chmod(0o600)
    with pytest.raises(CowtreeError, match="independent byte oracle failed"):
        verify(root=tmp_path, expected=before)


def cargo_fixture(root: Path) -> BuildSpec:
    root.mkdir()
    sources = {
        "Cargo.toml": '[workspace]\nmembers = ["app", "value"]\nresolver = "2"\n',
        ".gitignore": "target/\n.env\n",
        ".env": "private fixture token\n",
        ".cargo/config.toml": '[build]\nrustc-wrapper = "cowtree-fixture-missing-wrapper"\n',
        "app/Cargo.toml": (
            '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n'
            '[dependencies]\nvalue = { path = "../value" }\n'
        ),
        "app/src/main.rs": (
            'fn main() { println!("{}:{}", value::answer(), env!("FIXTURE_TAG")); }\n'
        ),
        "app/build.rs": (
            'fn main() {\nprintln!("cargo:rerun-if-env-changed=FIXTURE_TAG");\n'
            'println!("cargo:rustc-env=FIXTURE_TAG={}", '
            'std::env::var("FIXTURE_TAG").unwrap());\n}\n'
        ),
        "value/Cargo.toml": '[package]\nname = "value"\nversion = "0.1.0"\nedition = "2021"\n',
        "value/src/lib.rs": "pub fn answer() -> u32 { 42 }\n",
    }
    for name, text in sources.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
    for number in range(64):
        (root / f"fixture-{number:02}.txt").write_text(f"source witness {number}\n")
    cargo = shutil.which("cargo")
    assert cargo is not None, "Cargo must be installed for the mounted Rust qualification"
    runner = CommandRunner()
    cache = root.parent / "seed-cargo-home"
    cache.mkdir(exist_ok=True)
    runner.run(
        [cargo, "generate-lockfile", "--offline"],
        cwd=str(root),
        env={**os.environ, "CARGO_HOME": str(cache)},
    )
    for arguments in (
        ["init", "-q"],
        ["config", "user.email", "test@example.com"],
        ["config", "user.name", "Cowtree test"],
        ["config", "commit.gpgsign", "false"],
        ["add", "."],
        ["commit", "-qm", "Cargo fixture"],
    ):
        runner.run(["git", "-C", str(root), *arguments])
    spec = BuildSpec(
        cargo=Path(cargo),
        crate="app",
        executable="app",
        smoke=("{target}/debug/app",),
        expected_output="42:seed",
        environment={"FIXTURE_TAG": "seed", "CARGO_INCREMENTAL": "0"},
    )
    return spec


@pytest.mark.parametrize("prime_owned_source", [False, True])
def test_cargo_source_cache_and_build_environment_stay_private(
    tmp_path: Path, prime_owned_source: bool
) -> None:
    source = tmp_path / "source"
    spec = cargo_fixture(root=source)
    cargo_home = tmp_path / "seed-cargo-home"
    cargo_home.mkdir(exist_ok=True)
    seed = build(root=source, spec=spec, cargo_home=cargo_home, evidence=tmp_path / "seed")
    assert seed.compiled >= 2
    artifact = source / "target/debug/app"
    os.link(artifact, source / "target/hardlink-witness")
    assert artifact.stat().st_nlink > 1
    replacement = tmp_path / "replacement.rs"
    replacement.write_text("pub fn answer() -> u32 { 43 }\n")
    config = Config(
        root=tmp_path / "trial",
        source=source,
        binary=metadata_binary(),
        cargo_home=cargo_home,
        build=spec,
        derived_hardlinks=DerivedHardlinks.CLONE,
        prime_owned_source=prime_owned_source,
        mutation=Mutation(
            path="value/src/lib.rs",
            replacement=replacement,
            expected_output="43:edited",
            environment={"FIXTURE_TAG": "edited"},
        ),
    )
    result = run(config=config)
    assert result.source_entries > 64
    assert result.source_unchanged
    assert result.sibling_unchanged
    assert result.initial.smoke_output == "42:seed"
    assert (result.prime is not None) == prime_owned_source
    assert result.edited is not None
    assert result.edited.smoke_output == "43:edited"
    assert result.edited.compiled >= 2
    reopened = Workspace.open(root=config.root / "workspace")
    assert reopened.config.policy.derived_hardlinks is DerivedHardlinks.CLONE
    assert (config.root / "report.json").is_file()
    assert (config.root / "initial.jsonl").is_file()
    assert (config.root / "edited.jsonl").is_file()
    assert not (config.root / "sibling/.env").exists()
    assert not (config.root / "writer/.env").exists()
    before = inventory(root=source)
    del before[".env"]
    verify(root=config.root / "sibling", expected=before, cloned=True)


@pytest.mark.parametrize("prefix", ["value/src", "target"])
def test_cargo_import_refuses_hardlinks_in_source_and_derived_cache(
    tmp_path: Path, prefix: str
) -> None:
    source = tmp_path / "source"
    cargo_fixture(root=source)
    directory = source / prefix
    directory.mkdir(exist_ok=True)
    file = directory / "original"
    file.write_bytes(b"hard-link witness")
    os.link(file, directory / "alias")
    before = inventory(root=source)
    with pytest.raises(CowtreeError, match=r"hard.link"):
        Workspace.create(
            root=tmp_path / "workspace",
            source=source,
            binary=metadata_binary(),
            policy=PathPolicy(derived=("target",)),
        )
    assert not (tmp_path / "workspace").exists()
    assert not Workspace.recover_initialization(root=tmp_path / "workspace")
    verify(root=source, expected=before)
