"""Create and open a local workspace with one SQLite publication authority."""

from __future__ import annotations

from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import dataclass
import fcntl
import os
from pathlib import Path
import shutil
import tempfile

from pydantic import TypeAdapter

from cowtree.durable import sync_directory, write_record
from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.exec import CommandRunner
from cowtree.fs import doctor
from cowtree.git import GitRepository
from cowtree.initial_import import ImportProgress, InitialImport
from cowtree.metadata import Metadata
from cowtree.metadata_types import (
    Operation,
    Record,
    Tip,
)
from cowtree.nodes import Nodes
from cowtree.projection import GitProjection, ProjectionSpec
from cowtree.publication import publish_directory
from cowtree.submodules import admit
from cowtree.tree_types import PathPolicy
from cowtree.workspace_types import Node, WorkspaceConfig


DIRECTORIES = ("nodes", "leaves", "operations", "checks", "retained", "trash", "receipts")


class Initialization(Record):
    target: Path
    source: Path
    binary: Path
    git_directory: Path


@dataclass(frozen=True)
class Workspace:
    """Bind filesystem state and a stable Git common directory to a local authority."""

    root: Path
    config: WorkspaceConfig

    @property
    def repository(self) -> GitRepository:
        result = GitRepository(io=CommandRunner(), path=self.config.git_directory)
        return result

    @property
    def nodes(self) -> Nodes:
        result = Nodes(
            directory=self.root / "nodes",
            projection=GitProjection(self.repository),
            policy=self.config.policy,
        )
        return result

    @classmethod
    def open(cls, root: Path) -> Workspace:
        root = root.absolute()
        if root.is_symlink():
            raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "workspace root is a symlink")
        root = root.resolve()
        config = WorkspaceConfig.model_validate_json((root / "workspace.json").read_bytes())
        if config.location != root:
            raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "workspace identity mismatch")
        result = cls(root=root, config=config)
        return result

    @contextmanager
    def session(self, *, recover: bool = True) -> Iterator[Metadata]:
        """Serialize filesystem transitions and reject a replaced workspace directory."""
        identity = self.root.stat()
        with (self.root / "lock").open("a+b") as lock:
            fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
            try:
                current = self.root.lstat()
                if (identity.st_dev, identity.st_ino) != (current.st_dev, current.st_ino):
                    raise CowtreeError(
                        CowtreeErrorCode.INVALID_ARGUMENTS, "workspace directory changed"
                    )
                for name in DIRECTORIES:
                    directory = self.root / name
                    if directory.is_symlink() or not directory.is_dir():
                        raise CowtreeError(
                            CowtreeErrorCode.INVALID_ARGUMENTS,
                            f"workspace layout mismatch: {name}",
                        )
                with Metadata(
                    root=self.root / "authority",
                    binary=self.config.binary,
                    descriptors=(lock.fileno(),),
                ) as metadata:
                    if recover:
                        self.reconcile(metadata=metadata)
                    self.pin_origins(metadata=metadata)
                    yield metadata
                    self.pin_origins(metadata=metadata)
            finally:
                fcntl.flock(lock.fileno(), fcntl.LOCK_UN)

    def pin_origins(self, metadata: Metadata) -> None:
        """Retain authoritative origin bytes until their durable client records disappear."""
        from cowtree.leaves import Leaves

        objects = {
            entry.object for leaf in Leaves(self).records() for entry in leaf.origins.values()
        }
        for path in (self.root / "nodes").glob("*/node.json"):
            node = self.nodes.read(identity=path.parent.name)
            objects.update(entry.object for entry in node.origins.values())
        metadata.call(Operation.REPLACE_CLIENT_PINS, {"objects": sorted(objects)})

    def reconcile(self, metadata: Metadata) -> list[int]:
        """Recover client journals before admitting another filesystem transition."""
        from cowtree.captures import Captures
        from cowtree.collection import Collector
        from cowtree.leaves import Leaves
        from cowtree.lifecycle import Lifecycle
        from cowtree.publications import Publications
        from cowtree.seals import Seals
        from cowtree.views import Views

        Views(self).recover_locked(metadata=metadata)
        Seals(self).recover_locked(metadata=metadata)
        result = Leaves(self).recover_locked(metadata=metadata)
        Captures(self).recover_locked(metadata=metadata)
        Publications(self).recover_locked(metadata=metadata)
        Lifecycle(self).recover_locked(metadata=metadata)
        Collector(self).recover_locked(metadata=metadata)
        return result

    @staticmethod
    def staging(root: Path) -> Path:
        result = root.with_name(f".{root.name}.cowtree-init")
        return result

    @classmethod
    def create(cls, root: Path, source: Path, binary: Path, policy: PathPolicy) -> Workspace:
        root = root.absolute()
        repository = GitRepository.discover(io=CommandRunner(), source=source)
        if os.path.lexists(root) or root.resolve().is_relative_to(repository.path):
            raise CowtreeError(
                CowtreeErrorCode.INVALID_ARGUMENTS,
                "workspace must be absent and outside the source checkout",
            )
        root = root.resolve()
        policy = admit(repository=repository, policy=policy)
        if not doctor(path=root.parent).supported:
            raise CowtreeError(
                CowtreeErrorCode.COW_UNAVAILABLE, "workspace parent lacks native CoW"
            )
        staging = cls.staging(root=root)
        private = Path(tempfile.mkdtemp(prefix=f".{root.name}.cowtree-init-", dir=root.parent))
        try:
            with (private / "lock").open("a+b") as lock:
                fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
                write_record(
                    path=private / "initialization.json",
                    record=Initialization(
                        target=root,
                        source=repository.path,
                        binary=binary.resolve(),
                        git_directory=Path(
                            repository.capture(
                                args=["rev-parse", "--path-format=absolute", "--git-common-dir"]
                            ).removesuffix("\n")
                        ),
                    ),
                )
                publish_directory(source=private, target=staging)
                if os.path.lexists(root):
                    shutil.rmtree(staging)
                    sync_directory(path=root.parent)
                    raise CowtreeError(
                        CowtreeErrorCode.INVALID_ARGUMENTS, "workspace was initialized concurrently"
                    )
                cls.initialize(
                    staging=staging,
                    root=root,
                    repository=GitRepository(
                        io=CommandRunner(descriptors=(lock.fileno(),)), path=repository.path
                    ),
                    binary=binary,
                    policy=policy,
                )
                publish_directory(source=staging, target=root)
        finally:
            # Only this invocation's unpublished private directory is disposable.
            if private.exists():
                shutil.rmtree(private)
                sync_directory(path=root.parent)
        result = cls.open(root=root)
        return result

    @classmethod
    def initialize(
        cls, staging: Path, root: Path, repository: GitRepository, binary: Path, policy: PathPolicy
    ) -> None:
        for name in DIRECTORIES:
            (staging / name).mkdir()
        with repository.lock() as descriptor:
            locked = GitRepository(
                io=CommandRunner(descriptors=(*repository.io.descriptors, descriptor)),
                path=repository.path,
            )
            checkout = locked.snapshot(submodules=policy.submodules)
            if checkout.submodules != policy.pins:
                raise CowtreeError(
                    CowtreeErrorCode.HEAD_MISMATCH, "submodule pins changed during initialization"
                )
            nodes = Nodes(
                directory=staging / "nodes", projection=GitProjection(locked), policy=policy
            )
            node = nodes.seal(
                source=repository.path, projection=ProjectionSpec(parent=checkout.commit)
            )
            if locked.head() != checkout.commit:
                raise CowtreeError(
                    CowtreeErrorCode.HEAD_MISMATCH, "source HEAD changed during initialization"
                )
        common = repository.capture(
            args=["rev-parse", "--path-format=absolute", "--git-common-dir"]
        ).removesuffix("\n")
        config = WorkspaceConfig(
            location=root,
            source=repository.path,
            git_directory=Path(common),
            binary=binary.resolve(),
            policy=policy,
            initial=node.id,
            warm_tip=node.id,
        )
        with Metadata(
            root=staging / "authority", binary=binary, descriptors=repository.io.descriptors
        ) as metadata:
            metadata.call(Operation.INIT)
        write_record(path=staging / "import.json", record=config)
        importer = InitialImport(staging=staging, config=config)
        importer.run(descriptors=repository.io.descriptors)

    @classmethod
    def import_status(cls, root: Path) -> ImportProgress | None:
        """Observe acknowledged import progress on a published or initializing store."""
        root = root.resolve()
        if root.exists():
            workspace = cls.open(root=root)
            importer = InitialImport(staging=root, config=workspace.config)
        else:
            staging = cls.staging(root=root)
            config = WorkspaceConfig.model_validate_json((staging / "import.json").read_bytes())
            if config.location != root:
                raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "import target mismatch")
            importer = InitialImport(staging=staging, config=config)
        result = importer.status()
        return result

    @classmethod
    def recover_initialization(cls, root: Path) -> bool:
        """Publish a completed staging store, or discard an unacknowledged partial store."""
        if root.is_symlink():
            raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "workspace root is a symlink")
        root = root.resolve()
        if root.exists():
            cls.open(root=root)
            return True
        staging = cls.staging(root=root)
        try:
            lock = (staging / "lock").open("r+b")
        except FileNotFoundError:
            # A live creator may have published while this caller resolved the path.
            if root.exists():
                cls.open(root=root)
                return True
            if staging.exists():
                raise CowtreeError(
                    CowtreeErrorCode.INVALID_ARGUMENTS, "initialization lock is missing"
                ) from None
            return False
        with lock:
            fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
            if root.exists():
                cls.open(root=root)
                return True
            if not staging.exists():
                return False
            held = os.fstat(lock.fileno())
            current = (staging / "lock").stat()
            if (held.st_dev, held.st_ino) != (current.st_dev, current.st_ino):
                raise CowtreeError(
                    CowtreeErrorCode.INVALID_ARGUMENTS, "initializer changed; retry recovery"
                )
            record = Initialization.model_validate_json(
                (staging / "initialization.json").read_bytes()
            )
            if record.target != root:
                raise CowtreeError(
                    CowtreeErrorCode.INVALID_ARGUMENTS, "initialization target mismatch"
                )
            if (staging / "import.json").exists():
                config = WorkspaceConfig.model_validate_json((staging / "import.json").read_bytes())
                if config.location != root:
                    raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "import target mismatch")
                importer = InitialImport(staging=staging, config=config)
                importer.run(descriptors=(lock.fileno(),))
            if (staging / "workspace.json").exists():
                with Metadata(
                    root=staging / "authority", binary=record.binary, descriptors=(lock.fileno(),)
                ) as metadata:
                    metadata.call(Operation.TIP).decode("tip", TypeAdapter(Tip))
                publish_directory(source=staging, target=root)
                return True
            repository = GitRepository(
                io=CommandRunner(descriptors=(lock.fileno(),)), path=record.git_directory
            )
            projection = GitProjection(repository)
            for path in (staging / "nodes").glob("*/node.json"):
                node = Node.model_validate_json(path.read_bytes())
                if node.id != path.parent.name:
                    raise CowtreeError(
                        CowtreeErrorCode.INVALID_ARGUMENTS, "node ownership mismatch"
                    )
                projection.remove_ref(
                    reference=f"refs/cowtree/nodes/{node.id}", expected=node.git_commit
                )
                projection.remove_ref(
                    reference=f"refs/cowtree/tips/{node.id}", expected=node.git_commit
                )
            shutil.rmtree(staging)
            sync_directory(path=root.parent)
        return False
