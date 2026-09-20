"""Validate the exact prepared candidate in an isolated warm Git worktree."""

from __future__ import annotations

from dataclasses import dataclass
import fcntl
import hashlib
import os
from pathlib import Path
import subprocess
import sys
import uuid

from pydantic import TypeAdapter

from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.exec import CommandRunner
from cowtree.git import GitRepository
from cowtree.install import Change, Installation, InstallRecord
from cowtree.leaves import Leaves
from cowtree.lifecycle import Lifecycle
from cowtree.metadata_types import Candidate, Entry
from cowtree.nodes import Nodes
from cowtree.projection import GitProjection, ProjectionSpec
from cowtree.views import Views
from cowtree.workspace import Workspace
from cowtree.workspace_types import Leaf, Publication, Validation


@dataclass(frozen=True)
class Checks:
    workspace: Workspace

    def lock_path(self, candidate: Candidate) -> Path:
        result = (
            self.workspace.root
            / "checks"
            / f"{candidate.request.leaf}-{candidate.request.sequence}.lock"
        )
        return result

    def validate(
        self, identity: int, command: tuple[str, ...], timeout_seconds: int = 300
    ) -> Validation:
        if not command or timeout_seconds <= 0:
            raise CowtreeError(
                CowtreeErrorCode.INVALID_ARGUMENTS,
                "validation needs a command and positive timeout",
            )
        with self.workspace.session():
            pending = Leaves(self.workspace).read(identity=identity).pending
            if pending is None or pending.candidate is None or pending.parent_node is None:
                raise CowtreeError(
                    CowtreeErrorCode.INVALID_ARGUMENTS, "prepare a candidate before validation"
                )
        with self.lock_path(candidate=pending.candidate).open("a+b") as lock:
            try:
                fcntl.flock(lock.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError as error:
                raise CowtreeError(
                    CowtreeErrorCode.INVALID_ARGUMENTS, "validation is already running"
                ) from error
            leaf = Leaves(self.workspace).fork(
                path=self.workspace.root.parent / f".cowtree-check-{uuid.uuid4().hex}",
                node=pending.parent_node,
                check_candidate=pending.candidate,
            )
            checked = self.materialize(leaf=leaf, pending=pending, descriptor=lock.fileno())
            log = self.workspace.root / "checks" / f"{leaf.id}.log"
            self.run(
                command=command, leaf=checked, log=log, limits=(timeout_seconds, lock.fileno())
            )
            result = self.seal(
                identity=identity, leaf=checked, pending=pending, command=command, log=log
            )
        Lifecycle(self.workspace).drop(identity=checked.id, force=True, missing_ok=True)
        return result

    def manifest(self, candidate: Candidate) -> dict[str, Entry]:
        data = (self.workspace.root / "authority/objects" / candidate.root).read_bytes()
        if hashlib.sha256(data).hexdigest() != candidate.root:
            raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, "candidate manifest is corrupt")
        result = TypeAdapter(dict[str, Entry]).validate_json(data)
        return result

    def materialize(self, leaf: Leaf, pending: Publication, descriptor: int) -> Leaf:
        assert pending.candidate is not None
        assert pending.parent_node is not None
        with self.workspace.session() as metadata:
            current = Leaves(self.workspace).read(identity=pending.request.leaf).pending
            if current is None or current.candidate != pending.candidate:
                raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "candidate was superseded")
            manifest = self.manifest(candidate=pending.candidate)
            original = Views(self.workspace).current(leaf=leaf)
            changes = [
                Change(path=path, before=original.get(path), after=manifest.get(path))
                for path in sorted(set(original) | set(manifest))
                if original.get(path) != manifest.get(path)
            ]
            directory = self.workspace.root / "checks" / f"{leaf.id}-install"
            install = Installation.prepare(
                directory=directory,
                record=InstallRecord(root=leaf.path, changes=changes),
                objects=self.workspace.root / "authority/objects",
            )
            install.apply()
            repository = GitRepository(
                io=CommandRunner(descriptors=(*metadata.descriptors, descriptor)),
                path=self.workspace.config.git_directory,
            )
            parent = self.workspace.nodes.read(identity=pending.parent_node)
            commit = GitProjection(repository).create(
                tree=leaf.path, paths=tuple(manifest), parents=(parent.git_commit,)
            )
            target = GitRepository(io=repository.io, path=leaf.path)
            target.check_detached(expected=(leaf.git_head,))
            target.run(args=["-c", "core.fsync=all", "reset", "--mixed", "-q", commit])
            updated = leaf.model_copy(update={"origins": manifest, "git_head": commit})
            Leaves(self.workspace).save(leaf=updated)
            install.finish()
        return updated

    @staticmethod
    def run(command: tuple[str, ...], leaf: Leaf, log: Path, limits: tuple[int, int]) -> None:
        timeout_seconds, descriptor = limits
        if getattr(sys, "frozen", False):
            launcher = [sys.executable, "--cowtree-supervise"]
        else:
            launcher = [sys.executable, "-I", str(Path(__file__).with_name("supervise.py"))]
        with log.open("xb") as output:
            process = subprocess.Popen(  # noqa: S603
                [
                    *launcher,
                    "--timeout",
                    str(timeout_seconds),
                    "--descriptor",
                    str(descriptor),
                    "--",
                    *command,
                ],
                cwd=leaf.path,
                stdout=output,
                stderr=subprocess.STDOUT,
                stdin=subprocess.DEVNULL,
                pass_fds=(descriptor,),
            )
            code = process.wait()
            output.flush()
            os.fsync(output.fileno())
            if code != 0:
                raise CowtreeError(
                    CowtreeErrorCode.COMMAND_FAILED, f"candidate check exited {code}; {log}"
                )

    def seal(
        self, identity: int, leaf: Leaf, pending: Publication, command: tuple[str, ...], log: Path
    ) -> Validation:
        assert pending.candidate is not None
        assert pending.parent_node is not None
        with self.workspace.session() as metadata:
            owner = Leaves(self.workspace).read(identity=identity)
            if (
                owner.pending is None
                or owner.pending.candidate != pending.candidate
                or owner.pending.aborting
            ):
                raise CowtreeError(
                    CowtreeErrorCode.INVALID_ARGUMENTS, "validated candidate is no longer pending"
                )
            manifest = self.manifest(candidate=pending.candidate)
            if Views(self.workspace).current(leaf=leaf) != manifest:
                raise CowtreeError(
                    CowtreeErrorCode.DIRTY_SOURCE, "validation command changed source"
                )
            repository = GitRepository(
                io=CommandRunner(descriptors=metadata.descriptors),
                path=self.workspace.config.git_directory,
            )
            nodes = Nodes(
                directory=self.workspace.root / "nodes",
                projection=GitProjection(repository),
                policy=self.workspace.config.policy,
            )
            node = nodes.seal(
                source=leaf.path,
                origins=manifest,
                parent=pending.parent_node,
                projection=ProjectionSpec(reuse=leaf.git_head),
            )
            if node.source != manifest:
                raise CowtreeError(
                    CowtreeErrorCode.DIRTY_SOURCE, "candidate changed during validation capture"
                )
            result = Validation(
                candidate=pending.candidate, command=command, log=str(log), node=node.id
            )
            Leaves(self.workspace).save(
                leaf=owner.model_copy(
                    update={"pending": owner.pending.model_copy(update={"validation": result})}
                )
            )
        return result
