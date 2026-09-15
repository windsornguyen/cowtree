"""Parse cowtree commands and render operation results.

Entry points:
    main -- report domain failures with a process exit status.
    CowtreeCLI -- own command parsing and dispatch.
"""

from __future__ import annotations

import argparse
from pathlib import Path
import sys
from typing import TYPE_CHECKING

from inline_tests import test

from cowtree.core import add_worktree, inspect_path, list_all_worktrees, remove_worktree
from cowtree.errors import CowtreeError
from cowtree.types import Arguments, Command, WorktreeAddRequest


if TYPE_CHECKING:
    import pytest


# --- Entry point ---


def main(argv: list[str] | None = None) -> int:
    """Run a command, returning 0 for success, 1 for failure, or 2 for invalid usage."""
    args = list(sys.argv[1:] if argv is None else argv)
    cli = CowtreeCLI()
    try:
        code = cli.run(argv=args)
    except CowtreeError as error:
        print(f"cowtree: {error.code.value}: {error.message}", file=sys.stderr)
        code = 1
    return code


class CowtreeCLI:
    """Own the parsers and handlers for the supported command-line operations."""

    def __init__(self) -> None:
        """Configure each supported command without accepting arbitrary Git flags."""
        self.parser = argparse.ArgumentParser(prog="cowtree", allow_abbrev=False)
        commands = self.parser.add_subparsers(title="commands")
        parsers: dict[Command, argparse.ArgumentParser] = {}
        for command in Command:
            parser = commands.add_parser(command.value, allow_abbrev=False)
            parser.set_defaults(command=command)
            parsers[command] = parser

        self.add_parser = parsers[Command.ADD]
        self.add_parser.description = (
            "Clone source HEAD into a detached worktree, or create a new branch with -b."
        )
        mode = self.add_parser.add_mutually_exclusive_group()
        mode.add_argument("-b", dest="branch", metavar="NEW_BRANCH")
        mode.add_argument("--detach", "-d", action="store_true")
        self.add_parser.add_argument("--lock", action="store_true")
        self.add_parser.add_argument(
            "--reason", metavar="TEXT", help="lock reason; requires --lock"
        )
        self.add_parser.add_argument("path", type=Path)
        self.add_parser.add_argument("commitish", nargs="?", default="HEAD", metavar="commit-ish")

        parsers[Command.LIST].add_argument("--json", action="store_true")
        parsers[Command.LIST].add_argument("path", type=Path, nargs="?", metavar="source")
        parsers[Command.REMOVE].add_argument("--force", action="store_true")
        parsers[Command.REMOVE].add_argument("path", type=Path)
        parsers[Command.DOCTOR].add_argument("path", type=Path, nargs="?", metavar="directory")

    # --- Dispatch ---

    def run(self, argv: list[str]) -> int:
        """Parse explicit arguments and return the command's exit status."""
        options = Arguments()
        try:
            self.parser.parse_args(args=argv, namespace=options)
            match options.command:
                case Command.ADD:
                    code = self.add(options=options)
                case Command.LIST:
                    code = self.list_worktrees(options=options)
                case Command.REMOVE:
                    code = self.remove(options=options)
                case Command.DOCTOR:
                    code = self.doctor(options=options)
                case Command.HELP:
                    self.parser.print_help()
                    code = 0
        except SystemExit as error:
            assert isinstance(error.code, int)  # noqa: PT017
            code = error.code
        return code

    # --- Operations ---

    def add(self, options: Arguments) -> int:
        """Create a worktree and print its path after successful registration."""
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
        worktree = add_worktree(request=request)
        print(worktree.path)
        code = 0
        return code

    def list_worktrees(self, options: Arguments) -> int:
        """Print worktree metadata as text or a JSON array."""
        worktrees = list_all_worktrees(source=options.path)
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

    def remove(self, options: Arguments) -> int:
        """Remove a registered worktree under the requested dirty-file policy."""
        assert options.path is not None
        remove_worktree(path=options.path, force=options.force)
        code = 0
        return code

    def doctor(self, options: Arguments) -> int:
        """Probe native clone support and print the selected filesystem primitive."""
        path = Path.cwd() if options.path is None else options.path
        report = inspect_path(path=path)
        clone_tool = report.clone_tool
        if clone_tool is not None:
            print(f"{report.path}: {report.filesystem.value} via {clone_tool.value}")
            code = 0
            return code
        print(f"{report.path}: unsupported ({report.reason})")
        code = 1
        return code


# --- Tests ---


@test
def unknown_command_returns_usage(capsys: pytest.CaptureFixture[str]) -> None:
    cli = CowtreeCLI()
    code = cli.run(argv=["wat"])
    captured = capsys.readouterr()
    assert code == 2
    assert "invalid choice" in captured.err
    assert "usage: cowtree" in captured.err


@test
def remove_requires_exactly_one_path(capsys: pytest.CaptureFixture[str]) -> None:
    cli = CowtreeCLI()
    code = cli.run(argv=["remove", "--force"])
    captured = capsys.readouterr()
    assert code == 2
    assert "cowtree remove" in captured.err
