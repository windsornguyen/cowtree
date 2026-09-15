"""Run argv commands with reversible byte decoding and typed failures."""

from __future__ import annotations

from collections.abc import Mapping
import subprocess

from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.types import CommandResult


class CommandRunner:
    """Own subprocess execution without shell interpretation."""

    def run(
        self,
        argv: list[str],
        *,
        cwd: str | None = None,
        env: Mapping[str, str] | None = None,
        check: bool = True,
    ) -> CommandResult:
        """Run a command, preserving raw output bytes through surrogate escapes."""
        if not argv:
            raise CowtreeError(
                code=CowtreeErrorCode.INVALID_ARGUMENTS, message="argv must not be empty"
            )

        try:
            completed = subprocess.run(
                args=argv, cwd=cwd, env=env, capture_output=True, check=False
            )
        except ValueError as error:
            raise CowtreeError(
                CowtreeErrorCode.INVALID_ARGUMENTS, f"invalid command arguments: {error}"
            ) from error
        except OSError as error:
            raise CowtreeError(
                CowtreeErrorCode.COMMAND_FAILED, f"cannot run {argv[0]}: {error}"
            ) from error
        result = CommandResult(
            argv=argv,
            returncode=completed.returncode,
            stdout=completed.stdout.decode("utf-8", "surrogateescape"),
            stderr=completed.stderr.decode("utf-8", "surrogateescape"),
        )
        if check and completed.returncode != 0:
            raise CowtreeError(
                code=CowtreeErrorCode.COMMAND_FAILED,
                message=f"{' '.join(argv)} failed with exit code {completed.returncode}: "
                f"{result.stderr}{result.stdout}",
            )
        return result


# --- Tests ---

from inline_tests import test  # noqa: E402


@test
def rejects_empty_argv() -> None:
    import pytest

    runner = CommandRunner()
    with pytest.raises(CowtreeError) as error:
        runner.run(argv=[])
    assert error.value.code == CowtreeErrorCode.INVALID_ARGUMENTS


@test
def returns_argv_without_shell_parsing() -> None:
    runner = CommandRunner()
    result = runner.run(argv=["python3", "-c", "print('ok')"])
    assert result.argv == ["python3", "-c", "print('ok')"]
    assert result.stdout == "ok\n"
