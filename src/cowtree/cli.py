from __future__ import annotations

import argparse
from collections.abc import Callable
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
import sys
from typing import TYPE_CHECKING

from inline_tests import test

from cowtree.core import add_worktree, inspect_path, list_all_worktrees, remove_worktree
from cowtree.errors import CowtreeError
from cowtree.models import WorktreeAddRequest


if TYPE_CHECKING:
    import pytest


class _Command(str, Enum):
    ADD = "add"
    LIST = "list"
    REMOVE = "remove"
    DOCTOR = "doctor"
    HELP = "help"


@dataclass
class _Arguments(argparse.Namespace):
    command: _Command = _Command.HELP
    path: Path | None = None
    branch: str | None = None
    commitish: str = "HEAD"
    detach: bool = False
    lock: bool = False
    reason: str | None = None
    cow: bool = False
    no_checkout: bool = False
    json: bool = False
    force: bool = False


Handler = Callable[[_Arguments], int]


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
        self.parser = argparse.ArgumentParser(prog="cowtree", allow_abbrev=False)
        commands = self.parser.add_subparsers(title="commands")
        parsers: dict[_Command, argparse.ArgumentParser] = {}
        for command in _Command:
            parser = commands.add_parser(command.value, allow_abbrev=False)
            parser.set_defaults(command=command)
            parsers[command] = parser

        self.add_parser = parsers[_Command.ADD]
        self.add_parser.description = "Clone source HEAD into a detached worktree, or create a new branch with -b."
        mode = self.add_parser.add_mutually_exclusive_group()
        mode.add_argument("-b", dest="branch", metavar="NEW_BRANCH")
        mode.add_argument("--detach", "-d", action="store_true")
        self.add_parser.add_argument("--lock", action="store_true")
        self.add_parser.add_argument("--reason", metavar="TEXT", help="lock reason; requires --lock")
        self.add_parser.add_argument("--cow", action="store_true", help="compatibility marker; CoW is always required")
        self.add_parser.add_argument(
            "--no-checkout",
            action="store_true",
            help="compatibility marker; cowtree always populates files through CoW",
        )
        self.add_parser.add_argument("path", type=Path)
        self.add_parser.add_argument("commitish", nargs="?", default="HEAD", metavar="commit-ish")

        parsers[_Command.LIST].add_argument("--json", action="store_true")
        parsers[_Command.LIST].add_argument("path", type=Path, nargs="?", metavar="source")
        parsers[_Command.REMOVE].add_argument("--force", action="store_true")
        parsers[_Command.REMOVE].add_argument("path", type=Path)
        parsers[_Command.DOCTOR].add_argument("path", type=Path, nargs="?", metavar="directory")
        self.commands: dict[_Command, Handler] = {
            _Command.ADD: self.add,
            _Command.LIST: self.list_cmd,
            _Command.REMOVE: self.remove,
            _Command.DOCTOR: self.doctor,
            _Command.HELP: self.help,
        }

    def run(self, argv: list[str]) -> int:
        options = _Arguments()
        try:
            self.parser.parse_args(argv, namespace=options)
            code = self.commands[options.command](options)
        except SystemExit as error:
            assert isinstance(error.code, int)  # noqa: PT017
            code = error.code
        return code

    def add(self, options: _Arguments) -> int:
        assert options.path is not None
        try:
            request = WorktreeAddRequest(
                path=options.path,
                branch=options.branch,
                commitish=options.commitish,
                detach=options.detach,
                lock=options.lock,
                reason=options.reason,
            )
        except CowtreeError as error:
            self.add_parser.error(error.message)
        worktree = add_worktree(request)
        print(worktree.path)
        code = 0
        return code

    def list_cmd(self, options: _Arguments) -> int:
        worktrees = list_all_worktrees(options.path)
        if options.json:
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

    def remove(self, options: _Arguments) -> int:
        assert options.path is not None
        remove_worktree(options.path, force=options.force)
        code = 0
        return code

    def doctor(self, options: _Arguments) -> int:
        path = Path.cwd() if options.path is None else options.path
        report = inspect_path(path)
        clone_tool = report.clone_tool
        if clone_tool is not None:
            print(f"{report.path}: {report.filesystem.value} via {clone_tool.value}")
            code = 0
            return code
        print(f"{report.path}: unsupported ({report.reason})")
        code = 1
        return code

    def help(self, _options: _Arguments) -> int:
        self.parser.print_help()
        code = 0
        return code


@test
def unknown_command_returns_usage(capsys: pytest.CaptureFixture[str]) -> None:
    code = CowtreeCLI().run(["wat"])
    captured = capsys.readouterr()
    assert code == 2
    assert "invalid choice" in captured.err
    assert "usage: cowtree" in captured.err


@test
def remove_requires_exactly_one_path(capsys: pytest.CaptureFixture[str]) -> None:
    code = CowtreeCLI().run(["remove", "--force"])
    captured = capsys.readouterr()
    assert code == 2
    assert "cowtree remove" in captured.err
