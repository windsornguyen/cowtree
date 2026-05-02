from __future__ import annotations

from collections.abc import Callable
from pathlib import Path
import sys
from typing import TYPE_CHECKING

from inline_tests import test

from cowtree.core import add_worktree, inspect_path, list_all_worktrees, remove_worktree
from cowtree.errors import CowtreeError
from cowtree.models import WorktreeAddRequest


if TYPE_CHECKING:
    import pytest

Handler = Callable[[list[str]], int]


def main(argv: list[str] | None = None) -> int:
    args = list(sys.argv[1:] if argv is None else argv)
    cli = CowtreeCLI()
    try:
        code = cli.run(args)
    except CowtreeError as error:
        print(f"cowtree: {error.code.value}: {error.message}", file=sys.stderr)
        code = 1
    return code


class CowtreeCLI:
    def __init__(self) -> None:
        self.commands: dict[str, Handler] = {
            "add": self.add,
            "list": self.list_cmd,
            "remove": self.remove,
            "doctor": self.doctor,
            "help": self.help,
        }

    def run(self, argv: list[str]) -> int:
        command_name = argv[0] if argv else "help"
        command = self.commands.get(command_name)
        if command is None:
            print(f"cowtree: unknown command {command_name}", file=sys.stderr)
            code = self.help([])
            return code
        code = command(argv[1:])
        return code

    def add(self, argv: list[str]) -> int:
        worktree = add_worktree(WorktreeAddRequest(args=argv))
        print(worktree.path)
        code = 0
        return code

    def list_cmd(self, argv: list[str]) -> int:
        json_output = "--json" in argv
        source_args = [arg for arg in argv if arg != "--json"]
        source = Path(source_args[0]) if source_args else None
        worktrees = list_all_worktrees(source)
        if json_output:
            print("[" + ",".join(worktree.to_json_text() for worktree in worktrees) + "]")
        else:
            for worktree in worktrees:
                suffix = ""
                if worktree.branch:
                    suffix = f" {worktree.branch}"
                elif worktree.detached:
                    suffix = " detached"
                print(f"{worktree.path}{suffix}")
        code = 0
        return code

    def remove(self, argv: list[str]) -> int:
        force = "--force" in argv
        paths = [arg for arg in argv if arg != "--force"]
        if len(paths) != 1:
            print("usage: cowtree remove [--force] <path>", file=sys.stderr)
            code = 2
            return code
        remove_worktree(Path(paths[0]), force=force)
        code = 0
        return code

    def doctor(self, argv: list[str]) -> int:
        path = Path(argv[0]) if argv else Path.cwd()
        report = inspect_path(path)
        clone_tool = report.clone_tool
        if clone_tool is not None:
            print(f"{report.path}: {report.filesystem.value} via {clone_tool.value}")
            code = 0
            return code
        print(f"{report.path}: unsupported ({report.reason})")
        code = 1
        return code

    def help(self, _argv: list[str]) -> int:
        print(
            "usage: cowtree <command> [args]\n\n"
            "commands:\n"
            "  add      create a CoW worktree\n"
            "  list     list git worktrees\n"
            "  remove   remove a git worktree\n"
            "  doctor   show CoW support for a path\n"
        )
        code = 0
        return code


@test
def unknown_command_returns_usage(capsys: pytest.CaptureFixture[str]) -> None:
    code = CowtreeCLI().run(["wat"])
    captured = capsys.readouterr()
    assert code == 0
    assert "unknown command wat" in captured.err
    assert "usage: cowtree" in captured.out


@test
def remove_requires_exactly_one_path(capsys: pytest.CaptureFixture[str]) -> None:
    code = CowtreeCLI().run(["remove", "--force"])
    captured = capsys.readouterr()
    assert code == 2
    assert "cowtree remove" in captured.err
