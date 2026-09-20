"""Qualify Cargo cache inheritance and build isolation with an independent byte oracle."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import time
from typing import Literal

from pydantic import BaseModel, ConfigDict, Field, TypeAdapter

from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.exec import CommandRunner
from cowtree.leaves import Leaves
from cowtree.metadata_types import Record
from cowtree.tree_types import DerivedHardlinks, PathPolicy
from cowtree.workspace import Workspace


class Fingerprint(Record):
    kind: Literal["file", "symlink"]
    sha256: str
    size: int
    mode: int
    mtime_ns: int


class BuildSpec(Record):
    cargo: Path
    crate: str
    executable: str
    derived: str = "target"
    smoke: tuple[str, ...]
    expected_output: str
    environment: dict[str, str] = Field(default_factory=dict)
    timeout_seconds: int = 900


class Mutation(Record):
    path: str
    replacement: Path
    expected_output: str
    environment: dict[str, str] = Field(default_factory=dict)


class Config(Record):
    root: Path
    source: Path
    binary: Path
    cargo_home: Path
    build: BuildSpec
    mutation: Mutation | None = None
    derived_hardlinks: DerivedHardlinks = DerivedHardlinks.REJECT
    prime_owned_source: bool = False


class CargoMessage(BaseModel):
    model_config = ConfigDict(extra="ignore", strict=True)
    reason: str
    fresh: bool | None = None


class BuildReport(Record):
    seconds: float
    fresh: int
    compiled: int
    smoke_output: str


class Report(Record):
    source_entries: int
    source_sha256: str
    cargo_cache_copy_seconds: float
    import_seconds: float
    fork_seconds: float
    prime: BuildReport | None
    initial: BuildReport
    edited: BuildReport | None
    source_unchanged: Literal[True] = True
    sibling_unchanged: Literal[True] = True


class SetupReport(Record):
    cargo_cache_copy_seconds: float
    import_seconds: float
    fork_seconds: float


def inventory(root: Path, exclude: str | None = None) -> dict[str, Fingerprint]:
    """Hash bytes, file kinds and modes independently of Cowtree's tree scanner."""
    entries: dict[str, Fingerprint] = {}
    for directory, names, files in os.walk(root, followlinks=False):
        names[:] = [
            name
            for name in names
            if name != ".git" and (Path(directory) / name).relative_to(root).as_posix() != exclude
        ]
        for name in [*names, *files]:
            if name == ".git" or (Path(directory) / name).relative_to(root).as_posix() == exclude:
                continue
            path = Path(directory) / name
            metadata = path.lstat()
            if stat.S_ISDIR(metadata.st_mode):
                continue
            checksum = hashlib.sha256()
            if stat.S_ISLNK(metadata.st_mode):
                kind = "symlink"
                payload = os.fsencode(os.readlink(path))
                checksum.update(payload)
                size = len(payload)
            elif stat.S_ISREG(metadata.st_mode):
                kind = "file"
                with path.open("rb") as stream:
                    for block in iter(lambda: stream.read(1024 * 1024), b""):
                        checksum.update(block)
                size = metadata.st_size
            else:
                raise CowtreeError(
                    CowtreeErrorCode.INVALID_ARGUMENTS, f"unsupported fixture entry: {path}"
                )
            entries[path.relative_to(root).as_posix()] = Fingerprint(
                kind=kind,
                sha256=checksum.hexdigest(),
                size=size,
                mode=stat.S_IMODE(metadata.st_mode),
                mtime_ns=metadata.st_mtime_ns,
            )
    return entries


def digest(entries: dict[str, Fingerprint]) -> str:
    """Identify normalized contents; symlink timestamps are not a portability promise."""
    payload = {
        path: entry.model_dump(exclude={"mtime_ns"}) for path, entry in sorted(entries.items())
    }
    result = hashlib.sha256(json.dumps(payload, sort_keys=True).encode()).hexdigest()
    return result


def clone_entries(
    source: Path, before: dict[str, Fingerprint], derived: str
) -> dict[str, Fingerprint]:
    """Model eligible paths with Git's ignore rules, independently of Cowtree's scanner."""
    runner = CommandRunner()
    visible = runner.run(
        ["git", "-C", str(source), "ls-files", "--cached", "--others", "--exclude-standard", "-z"]
    )
    paths = set(visible.stdout.split("\0"))
    result = {
        name: entry
        for name, entry in before.items()
        if name in paths or name == derived or name.startswith(derived + "/")
    }
    return result


def verify(
    root: Path,
    expected: dict[str, Fingerprint],
    *,
    cloned: bool = False,
    exclude: str | None = None,
) -> None:
    actual = inventory(root=root, exclude=exclude)
    changed = sorted(expected.keys() ^ actual.keys())
    for path in expected.keys() & actual.keys():
        before, after = expected[path], actual[path]
        if cloned and before.kind == "symlink":
            before = before.model_copy(update={"mtime_ns": after.mtime_ns})
        if before != after:
            changed.append(path)
    if changed:
        raise CowtreeError(
            CowtreeErrorCode.COMMAND_FAILED,
            f"independent byte oracle failed at {root}: {changed[:8]}",
        )


def child(root: Path, relative: str) -> Path:
    path = root / relative
    if (
        Path(relative).is_absolute()
        or ".." in Path(relative).parts
        or ".git" in Path(relative).parts
    ):
        raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, f"unsafe relative path: {relative}")
    if not path.resolve().is_relative_to(root.resolve()) or path == root:
        raise CowtreeError(
            CowtreeErrorCode.INVALID_ARGUMENTS, f"path escapes workspace: {relative}"
        )
    return path


def private_cargo_home(source: Path, target: Path) -> None:
    """Copy downloaded dependencies; never give Cargo a writable user cache or credentials."""
    if not source.is_dir():
        raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, f"Cargo home missing: {source}")
    target.mkdir()
    for name in ("registry", "git"):
        directory = source / name
        if directory.exists():
            shutil.copytree(directory, target / name)


def build_environment(root: Path, spec: BuildSpec, cargo_home: Path) -> dict[str, str]:
    target = child(root=root, relative=spec.derived)
    environment = dict(os.environ)
    for key in (
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "CARGO_BUILD_RUSTC_WRAPPER",
        "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
    ):
        environment.pop(key, None)
    environment.update(spec.environment)
    environment.update(
        CARGO_HOME=str(cargo_home),
        CARGO_TARGET_DIR=str(target),
        CARGO_BUILD_BUILD_DIR=str(target),
        SCCACHE_DISABLE="1",
        RUSTC_WRAPPER="",
        RUSTC_WORKSPACE_WRAPPER="",
    )
    return environment


def build(root: Path, spec: BuildSpec, cargo_home: Path, evidence: Path) -> BuildReport:
    target = child(root=root, relative=spec.derived)
    environment = build_environment(root=root, spec=spec, cargo_home=cargo_home)
    command = [
        str(spec.cargo),
        "build",
        "--offline",
        "--locked",
        "--package",
        spec.crate,
        "--bin",
        spec.executable,
        "--message-format=json",
        "-j",
        "2",
    ]
    started = time.perf_counter()
    with (
        evidence.with_suffix(".jsonl").open("w") as output,
        evidence.with_suffix(".log").open("w") as errors,
    ):
        subprocess.run(  # noqa: S603 -- the caller selects the Cargo executable and fixture.
            command,
            cwd=root,
            env=environment,
            stdout=output,
            stderr=errors,
            check=True,
            timeout=spec.timeout_seconds,
        )
    elapsed = time.perf_counter() - started
    messages = [
        CargoMessage.model_validate_json(line)
        for line in evidence.with_suffix(".jsonl").read_text().splitlines()
    ]
    artifacts = [message for message in messages if message.reason == "compiler-artifact"]
    if not artifacts or any(message.fresh is None for message in artifacts):
        raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, "Cargo omitted artifact freshness")
    smoke = [
        argument.replace("{workspace}", str(root)).replace("{target}", str(target))
        for argument in spec.smoke
    ]
    observed = subprocess.run(  # noqa: S603 -- explicit smoke argv, without a shell.
        smoke,
        cwd=root,
        env=environment,
        capture_output=True,
        text=True,
        check=True,
        timeout=spec.timeout_seconds,
    ).stdout.strip()
    if observed != spec.expected_output:
        raise CowtreeError(
            CowtreeErrorCode.COMMAND_FAILED,
            f"smoke output differs: {observed!r} != {spec.expected_output!r}",
        )
    result = BuildReport(
        seconds=elapsed,
        fresh=sum(message.fresh is True for message in artifacts),
        compiled=sum(message.fresh is False for message in artifacts),
        smoke_output=observed,
    )
    return result


def run(config: Config) -> Report:
    """Retain all owned scratch data and receipts for inspection, including on failure."""
    if config.root.resolve().is_relative_to(config.source.resolve()):
        raise CowtreeError(
            CowtreeErrorCode.INVALID_ARGUMENTS, "scratch root must be outside source"
        )
    config.root.mkdir()
    cargo_home = config.root / "cargo-home"
    started = time.perf_counter()
    private_cargo_home(source=config.cargo_home, target=cargo_home)
    cache_seconds = time.perf_counter() - started
    prime = None
    if config.prime_owned_source:
        source_before = inventory(root=config.source, exclude=config.build.derived)
        prime = build(
            root=config.source,
            spec=config.build,
            cargo_home=cargo_home,
            evidence=config.root / "prime",
        )
        verify(root=config.source, expected=source_before, exclude=config.build.derived)
        (config.root / "prime.json").write_text(prime.model_dump_json(indent=2) + "\n")
    before = inventory(root=config.source)
    expected = clone_entries(source=config.source, before=before, derived=config.build.derived)
    manifest = TypeAdapter(dict[str, Fingerprint])
    (config.root / "source-manifest.json").write_bytes(manifest.dump_json(before, indent=2))
    (config.root / "clone-manifest.json").write_bytes(manifest.dump_json(expected, indent=2))
    started = time.perf_counter()
    workspace = Workspace.create(
        root=config.root / "workspace",
        source=config.source,
        binary=config.binary,
        policy=PathPolicy(
            derived=(config.build.derived,), derived_hardlinks=config.derived_hardlinks
        ),
    )
    import_seconds = time.perf_counter() - started
    started = time.perf_counter()
    leaves = Leaves(workspace=workspace)
    writer = leaves.fork(path=config.root / "writer")
    sibling = leaves.fork(path=config.root / "sibling")
    fork_seconds = time.perf_counter() - started
    setup = SetupReport(
        cargo_cache_copy_seconds=cache_seconds,
        import_seconds=import_seconds,
        fork_seconds=fork_seconds,
    )
    (config.root / "setup.json").write_text(setup.model_dump_json(indent=2) + "\n")
    verify(root=writer.path, expected=expected, cloned=True)
    verify(root=sibling.path, expected=expected, cloned=True)
    sibling_before = inventory(root=sibling.path)
    initial = build(
        root=writer.path, spec=config.build, cargo_home=cargo_home, evidence=config.root / "initial"
    )
    verify(root=config.source, expected=before)
    verify(root=sibling.path, expected=sibling_before)
    edited = None
    if config.mutation is not None:
        mutation = config.mutation
        path = child(root=writer.path, relative=mutation.path)
        if (
            mutation.path not in expected
            or path.is_symlink()
            or not path.is_file()
            or path.is_relative_to(writer.path / config.build.derived)
        ):
            raise CowtreeError(
                CowtreeErrorCode.INVALID_ARGUMENTS, "mutation must replace a regular source file"
            )
        path.write_bytes(mutation.replacement.read_bytes())
        spec = config.build.model_copy(
            update={
                "expected_output": mutation.expected_output,
                "environment": {**config.build.environment, **mutation.environment},
            }
        )
        edited = build(
            root=writer.path, spec=spec, cargo_home=cargo_home, evidence=config.root / "edited"
        )
        verify(root=config.source, expected=before)
        verify(root=sibling.path, expected=sibling_before)
    result = Report(
        source_entries=len(before),
        source_sha256=digest(entries=before),
        cargo_cache_copy_seconds=cache_seconds,
        import_seconds=import_seconds,
        fork_seconds=fork_seconds,
        prime=prime,
        initial=initial,
        edited=edited,
    )
    (config.root / "report.json").write_text(result.model_dump_json(indent=2) + "\n")
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("root", "source", "binary", "cargo", "cargo-home"):
        parser.add_argument(f"--{name}", required=True, type=Path)
    parser.add_argument("--crate", required=True)
    parser.add_argument("--executable", required=True)
    parser.add_argument("--derived", default="target")
    parser.add_argument(
        "--prime-owned-source",
        action="store_true",
        help="explicitly build a disposable source fixture before freezing its cache",
    )
    parser.add_argument(
        "--derived-hardlinks",
        choices=[mode.value for mode in DerivedHardlinks],
        type=DerivedHardlinks,
        default=DerivedHardlinks.REJECT,
    )
    parser.add_argument(
        "--smoke-command", required=True, help="JSON argv; {workspace} and {target} expand"
    )
    parser.add_argument("--expect-output", required=True)
    parser.add_argument("--environment", default="{}", help="JSON build environment overrides")
    parser.add_argument("--edit-path")
    parser.add_argument("--edit-content", type=Path)
    parser.add_argument("--edit-expect-output")
    args = parser.parse_args()
    mutation = None
    if any(
        value is not None for value in (args.edit_path, args.edit_content, args.edit_expect_output)
    ):
        if any(
            value is None for value in (args.edit_path, args.edit_content, args.edit_expect_output)
        ):
            parser.error(
                "--edit-path, --edit-content and --edit-expect-output must be supplied together"
            )
        mutation = Mutation(
            path=args.edit_path,
            replacement=args.edit_content.resolve(),
            expected_output=args.edit_expect_output,
        )
    spec = BuildSpec(
        cargo=args.cargo.absolute(),
        crate=args.crate,
        executable=args.executable,
        derived=args.derived,
        smoke=TypeAdapter(tuple[str, ...]).validate_json(args.smoke_command),
        expected_output=args.expect_output,
        environment=TypeAdapter(dict[str, str]).validate_json(args.environment),
    )
    result = run(
        config=Config(
            root=args.root.resolve(),
            source=args.source.resolve(),
            binary=args.binary.resolve(),
            cargo_home=args.cargo_home.resolve(),
            build=spec,
            mutation=mutation,
            derived_hardlinks=args.derived_hardlinks,
            prime_owned_source=args.prime_owned_source,
        )
    )
    print(result.model_dump_json(indent=2))


if __name__ == "__main__":
    main()
