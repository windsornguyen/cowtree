"""Identify the installed CLI from distribution metadata, never the caller's Git HEAD."""

from dataclasses import dataclass
from importlib.metadata import PackageNotFoundError, distribution
from pathlib import Path
from typing import Protocol

from inline_tests import test
from pydantic import BaseModel, ConfigDict, ValidationError

from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.exec import CommandRunner


class Package(Protocol):
    """Read the installed package's version and optional PEP 610 provenance."""

    @property
    def version(self) -> str: ...

    def read_text(self, filename: str) -> str | None: ...


class VcsOrigin(BaseModel):
    """Commit recorded by the installer, not inferred from adjacent source files."""

    commit_id: str


class Origin(BaseModel):
    """Ignore URLs and unrelated installer metadata in the public version result."""

    vcs_info: VcsOrigin | None = None


class VersionInfo(BaseModel):
    """Unknown build provenance is null, not a guessed revision."""

    model_config = ConfigDict(frozen=True, extra="forbid")
    version: str
    revision: str | None = None

    @classmethod
    def read(cls, io: Package) -> "VersionInfo":
        raw = io.read_text("direct_url.json")
        revision = None
        if raw is not None:
            try:
                origin = Origin.model_validate_json(raw)
            except ValidationError as error:
                raise CowtreeError(
                    CowtreeErrorCode.COMMAND_FAILED, "invalid installed package provenance"
                ) from error
            if origin.vcs_info is not None:
                revision = origin.vcs_info.commit_id
        result = cls(version=io.version, revision=revision)
        return result

    @classmethod
    def installed(cls) -> "VersionInfo":
        try:
            package = distribution("cowtree")
        except PackageNotFoundError as error:
            raise CowtreeError(
                CowtreeErrorCode.COMMAND_FAILED, "cowtree package metadata missing; install cowtree"
            ) from error
        result = cls.read(io=package)
        return result


class BackendInfo(VersionInfo):
    """Identify the configured executable separately from the installed CLI."""

    path: Path


class WorkspaceVersions(BaseModel):
    """A read-only version query never starts the metadata service or opens its store."""

    cli: VersionInfo
    backend: BackendInfo

    @classmethod
    def read(cls, io: CommandRunner, binary: Path) -> "WorkspaceVersions":
        # Two arguments make older binaries reject usage instead of opening a '--version' store.
        res = io.run(argv=[str(binary), "--version", "--json"])
        try:
            version = VersionInfo.model_validate_json(res.stdout)
        except ValidationError as error:
            raise CowtreeError(
                CowtreeErrorCode.COMMAND_FAILED,
                "metadata executable returned invalid build identity",
            ) from error
        result = cls(
            cli=VersionInfo.installed(),
            backend=BackendInfo(
                path=binary,
                version=version.version,
                revision=version.revision,
            ),
        )
        return result


# --- Tests ---


@test
def provenance_is_explicit_and_excludes_origin_urls() -> None:
    @dataclass(frozen=True)
    class PackageFixture:
        version: str = "1.2.3"
        origin: str | None = None

        def read_text(self, filename: str) -> str | None:
            assert filename == "direct_url.json"
            result = self.origin
            return result

    result = VersionInfo.read(io=PackageFixture())
    assert result.version == "1.2.3"
    assert result.revision is None
    io = PackageFixture(
        origin='{"url":"https://example.invalid/repo","vcs_info":{"commit_id":"abc"}}'
    )
    result = VersionInfo.read(io=io)
    assert result.revision == "abc"
    assert "example.invalid" not in result.model_dump_json()
