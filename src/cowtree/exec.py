from __future__ import annotations

from collections.abc import Mapping
import subprocess

from inline_tests import test

from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.models import CommandResult


class CommandRunner:
    def run(
        self,
        argv: list[str],
        *,
        cwd: str | None = None,
        env: Mapping[str, str] | None = None,
        check: bool = True,
    ) -> CommandResult:
        if not argv:
            raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "argv must not be empty")

        completed = subprocess.run(argv, cwd=cwd, env=env, capture_output=True, text=True, check=False)
        result = CommandResult(
            argv=argv,
            returncode=completed.returncode,
            stdout=completed.stdout,
            stderr=completed.stderr,
        )
        if check and completed.returncode != 0:
            raise CowtreeError(
                CowtreeErrorCode.COMMAND_FAILED,
                f"{' '.join(argv)} failed with exit code {completed.returncode}: {completed.stderr}{completed.stdout}",
            )
        return result


@test
def rejects_empty_argv() -> None:
    import pytest

    runner = CommandRunner()
    with pytest.raises(CowtreeError) as error:
        runner.run([])
    assert error.value.code == CowtreeErrorCode.INVALID_ARGUMENTS


@test
def returns_argv_without_shell_parsing() -> None:
    runner = CommandRunner()
    result = runner.run(["python3", "-c", "print('ok')"])
    assert result.argv == ["python3", "-c", "print('ok')"]
    assert result.stdout == "ok\n"
