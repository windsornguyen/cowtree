"""JSON commands for the managed workspace lifecycle."""

import argparse
from dataclasses import dataclass, field
from enum import Enum
from pathlib import Path
import sys
from typing import NoReturn

from pydantic import JsonValue, TypeAdapter, ValidationError

from cowtree.batches import Batches
from cowtree.captures import Captures
from cowtree.checks import Checks
from cowtree.collection import Collector
from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.exec import CommandRunner
from cowtree.leaves import Leaves
from cowtree.lifecycle import Lifecycle
from cowtree.metadata import MetadataError
from cowtree.metadata_types import BatchCandidate, Record, Request
from cowtree.publications import Publications
from cowtree.resolutions import Choice, Resolutions
from cowtree.seals import Seals
from cowtree.tree_types import PathPolicy
from cowtree.version import WorkspaceVersions
from cowtree.views import Views
from cowtree.workspace import Workspace


class Action(str, Enum):
    VERSION = "version"
    INIT = "init"
    LIST = "list"
    FORK = "fork"
    ACQUIRE = "acquire"
    SYNC = "sync"
    DISCARD = "discard"
    SEAL = "seal"
    RETAIN = "retain"
    RELEASE = "release"
    CAPTURE = "capture"
    PREPARE = "prepare"
    CHECK = "check"
    COMMIT = "commit"
    RESULT = "result"
    ABORT = "abort"
    DROP = "drop"
    COLLECT = "collect"
    RECOVER = "recover"
    LOG = "log"
    RESOLVE = "resolve"
    PREPARE_BATCH = "prepare-batch"
    CHECK_BATCH = "check-batch"
    COMMIT_BATCH = "commit-batch"


@dataclass
class Options(argparse.Namespace):
    root: Path = Path(".")
    action: Action = Action.LIST
    source: Path = Path(".")
    binary: Path = Path(".")
    path: Path = Path(".")
    node: str | None = None
    identity: int = 0
    sequence: int = 0
    timeout: int = 300
    force: bool = False
    derived: list[str] = field(default_factory=list)
    ephemeral: list[str] = field(default_factory=list)
    paths: list[str] = field(default_factory=list)
    command: list[str] = field(default_factory=list)
    identities: list[int] = field(default_factory=list)
    choices: Path = Path(".")
    candidate: Path = Path(".")


class Parser(argparse.ArgumentParser):
    def error(self, message: str) -> NoReturn:
        raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, message)


def parser() -> Parser:
    result = Parser(prog="cowtree workspace", allow_abbrev=False)
    result.add_argument("--root", type=Path, required=True, help="managed store directory")
    commands = result.add_subparsers(dest="action", required=True)
    for action in Action:
        command = commands.add_parser(action.value, allow_abbrev=False)
        command.set_defaults(action=action)
        if action in (
            Action.ACQUIRE,
            Action.SYNC,
            Action.DISCARD,
            Action.SEAL,
            Action.CAPTURE,
            Action.PREPARE,
            Action.CHECK,
            Action.COMMIT,
            Action.RESULT,
            Action.ABORT,
            Action.DROP,
            Action.RESOLVE,
        ):
            command.add_argument("identity", type=int, help="leaf identifier")
        if action in (Action.ACQUIRE, Action.DISCARD):
            command.add_argument("paths", nargs="+")
        if action in (Action.RETAIN, Action.RELEASE):
            command.add_argument("node", help="checkpoint identifier")
        if action is Action.INIT:
            command.add_argument("--source", type=Path, required=True)
            command.add_argument("--binary", type=Path, required=True)
            command.add_argument("--derived", action="append", default=[])
            command.add_argument("--ephemeral", action="append", default=[])
        if action is Action.FORK:
            command.add_argument("path", type=Path)
            command.add_argument("--node")
        if action in (Action.CHECK, Action.CHECK_BATCH):
            command.add_argument("--timeout", type=int, default=300)
            command.epilog = "Pass the check command after --, for example: -- make test"
        if action in (Action.CHECK_BATCH, Action.COMMIT_BATCH):
            command.add_argument("--candidate", type=Path, required=True)
        if action is Action.PREPARE_BATCH:
            command.add_argument("identities", type=int, nargs="+")
        if action is Action.RESOLVE:
            command.add_argument("--choices", type=Path, required=True)
        if action is Action.RESULT:
            command.add_argument("sequence", type=int)
        if action is Action.DROP:
            command.add_argument("--force", action="store_true")
    return result


def parse(argv: list[str]) -> Options:
    """Separate child argv before parsing so child flags cannot change CLI options."""
    command_parser = parser()
    options = Options()
    command_parser.parse_known_args(argv, namespace=options)
    if options.action in (Action.CHECK, Action.CHECK_BATCH):
        if "--" not in argv:
            command_parser.error("check command must follow --")
        boundary = argv.index("--")
        options = Options()
        command_parser.parse_args(argv[:boundary], namespace=options)
        options.command = argv[boundary + 1 :]
    else:
        command_parser.parse_args(argv, namespace=options)
    return options


@dataclass(frozen=True)
class Dispatch:
    workspace: Workspace

    def execute(self, options: Options) -> JsonValue:
        if options.action is Action.VERSION:
            versions = WorkspaceVersions.read(
                io=CommandRunner(), binary=self.workspace.config.binary
            )
            result = versions.model_dump(mode="json")
            return result
        if options.action in (Action.PREPARE_BATCH, Action.CHECK_BATCH, Action.COMMIT_BATCH):
            batch = self.batch(options=options)
            result = batch.model_dump(mode="json")
            return result
        if options.action in (
            Action.CAPTURE,
            Action.PREPARE,
            Action.CHECK,
            Action.COMMIT,
            Action.RESULT,
            Action.ABORT,
        ):
            value = self.publication(options=options)
            result = None if value is None else value.model_dump(mode="json")
            return result
        if options.action is Action.RESOLVE:
            choices = TypeAdapter(dict[str, Choice]).validate_json(options.choices.read_bytes())
            resolutions = Resolutions(self.workspace)
            leaf = resolutions.resolve(identity=options.identity, choices=choices)
            result = leaf.model_dump(mode="json")
            return result
        value = self.lifecycle(options=options)
        if isinstance(value, Record):
            result = value.model_dump(mode="json")
        elif isinstance(value, list):
            result = [
                item.model_dump(mode="json") if isinstance(item, Record) else item for item in value
            ]
        else:
            result = None
        return result

    def lifecycle(self, options: Options) -> Record | list[Record] | list[int] | None:
        leaves = Leaves(self.workspace)
        views = Views(self.workspace)
        seals = Seals(self.workspace)
        value: Record | list[Record] | list[int] | None
        match options.action:
            case Action.LIST:
                with self.workspace.session():
                    value = list(leaves.records())
            case Action.FORK:
                value = leaves.fork(path=options.path, node=options.node)
            case Action.ACQUIRE:
                value = views.acquire(identity=options.identity, paths=tuple(options.paths))
            case Action.SYNC:
                value = views.sync(identity=options.identity)
            case Action.DISCARD:
                value = views.discard(identity=options.identity, paths=tuple(options.paths))
            case Action.SEAL:
                value = seals.seal(identity=options.identity)
            case Action.RETAIN | Action.RELEASE:
                assert options.node is not None
                if options.action is Action.RETAIN:
                    seals.retain(identity=options.node)
                else:
                    seals.release(identity=options.node)
                value = None
            case Action.DROP:
                Lifecycle(self.workspace).drop(identity=options.identity, force=options.force)
                value = None
            case Action.COLLECT:
                value = Collector(self.workspace).collect()
            case Action.RECOVER:
                with self.workspace.session(recover=False) as metadata:
                    value = self.workspace.reconcile(metadata=metadata)
            case Action.LOG:
                with self.workspace.session():
                    value = [
                        self.workspace.nodes.read(identity=path.parent.name)
                        for path in sorted((self.workspace.root / "nodes").glob("*/node.json"))
                    ]
            case _:
                raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "invalid lifecycle command")
        return value

    def publication(self, options: Options) -> Record | None:
        publications = Publications(self.workspace)
        result: Record | None
        match options.action:
            case Action.CAPTURE:
                result = Captures(self.workspace).capture(identity=options.identity)
            case Action.PREPARE:
                result = publications.prepare(identity=options.identity)
            case Action.CHECK:
                result = Checks(self.workspace).validate(
                    identity=options.identity,
                    command=check_command(options=options),
                    timeout_seconds=options.timeout,
                )
            case Action.COMMIT:
                with self.workspace.session():
                    leaf = Leaves(self.workspace).read(identity=options.identity)
                    pending = leaf.pending
                    if pending is None or pending.candidate is None:
                        raise CowtreeError(
                            CowtreeErrorCode.INVALID_ARGUMENTS, "prepare a candidate first"
                        )
                    candidate = pending.candidate
                result = publications.commit(candidate=candidate)
            case Action.RESULT:
                result = publications.result(
                    request=Request(leaf=options.identity, sequence=options.sequence)
                )
            case Action.ABORT:
                Captures(self.workspace).abort(identity=options.identity)
                result = None
            case _:
                raise CowtreeError(
                    CowtreeErrorCode.INVALID_ARGUMENTS, "invalid publication command"
                )
        return result

    def batch(self, options: Options) -> Record:
        batches = Batches(self.workspace)
        if options.action is Action.PREPARE_BATCH:
            result = batches.prepare(identities=tuple(options.identities))
            return result
        candidate = BatchCandidate.model_validate_json(options.candidate.read_bytes())
        if options.action is Action.CHECK_BATCH:
            result = batches.validate(
                candidate=candidate,
                command=check_command(options=options),
                timeout_seconds=options.timeout,
            )
        else:
            result = batches.commit(candidate=candidate)
        return result


def check_command(options: Options) -> tuple[str, ...]:
    command = options.command
    if command[:1] == ["--"]:
        command = command[1:]
    result = tuple(command)
    return result


def run(argv: list[str]) -> int:
    """Return structured failures without swallowing unexpected programming errors."""
    adapter = TypeAdapter(JsonValue)
    try:
        options = parse(argv=argv)
        if options.action is Action.INIT:
            workspace = Workspace.create(
                root=options.root,
                source=options.source,
                binary=options.binary,
                policy=PathPolicy(
                    derived=tuple(options.derived), ephemeral=tuple(options.ephemeral)
                ),
            )
            value = workspace.config.model_dump(mode="json")
        elif options.action is Action.RECOVER and not options.root.exists():
            value = Workspace.recover_initialization(root=options.root)
        else:
            workspace = Workspace.open(root=options.root)
            dispatch = Dispatch(workspace=workspace)
            value = dispatch.execute(options=options)
        output: JsonValue = {"status": "ok", "kind": options.action.value, "value": value}
        print(adapter.dump_json(output).decode())
        return 0
    except SystemExit as error:
        assert isinstance(error.code, int)  # noqa: PT017 - argparse exit, not a test assertion.
        return error.code
    except (CowtreeError, OSError, ValidationError) as error:
        if isinstance(error, MetadataError):
            code = error.reason.value
        elif isinstance(error, CowtreeError):
            code = error.code.value
        elif isinstance(error, ValidationError):
            code = "invalid_arguments"
        else:
            code = "io"
        output = {"status": "error", "code": code, "message": str(error)}
        if isinstance(error, MetadataError):
            output["retry_action"] = error.retry_action.value
            output["details"] = error.details.model_dump(mode="json")
        print(adapter.dump_json(output).decode(), file=sys.stderr)
        result = 2 if code == "invalid_arguments" else 1
        return result
