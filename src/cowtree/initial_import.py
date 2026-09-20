"""Resume initial source import from one immutable client snapshot."""

from dataclasses import dataclass
from pathlib import Path

from pydantic import NonNegativeInt, TypeAdapter

from cowtree.durable import sync_directory, write_record
from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.exec import CommandRunner
from cowtree.git import GitRepository
from cowtree.metadata import Metadata
from cowtree.metadata_types import Digest, Operation, Record, Tip
from cowtree.nodes import Nodes
from cowtree.projection import GitProjection
from cowtree.workspace_types import WorkspaceConfig


class ImportProgress(Record):
    root: Digest
    completed: NonNegativeInt
    total: NonNegativeInt
    complete: bool


@dataclass(frozen=True)
class InitialImport:
    staging: Path
    config: WorkspaceConfig

    def run(self, descriptors: tuple[int, ...]) -> None:
        """Advance bounded durable chunks; publish config only after epoch completion."""
        repository = GitRepository(
            io=CommandRunner(descriptors=descriptors), path=self.config.git_directory
        )
        nodes = Nodes(
            directory=self.staging / "nodes",
            projection=GitProjection(repository),
            policy=self.config.policy,
        )
        node = nodes.read(identity=self.config.initial)
        nodes.verify(node=node)
        with Metadata(
            root=self.staging / "authority", binary=self.config.binary, descriptors=descriptors
        ) as metadata:
            progress = metadata.call(
                Operation.BEGIN_IMPORT,
                {
                    "manifest": {
                        path: entry.model_dump(mode="json") for path, entry in node.source.items()
                    }
                },
            ).decode("import_progress", TypeAdapter(ImportProgress))
            while progress.completed < progress.total:
                previous = progress.completed
                progress = metadata.call(
                    Operation.IMPORT_CHUNK,
                    {
                        "root": progress.root,
                        "source": str(nodes.directory / node.id / "tree"),
                    },
                ).decode("import_progress", TypeAdapter(ImportProgress))
                if progress.completed <= previous:
                    raise CowtreeError(
                        CowtreeErrorCode.COMMAND_FAILED, "initial import made no progress"
                    )
            progress = metadata.call(Operation.FINISH_IMPORT, {"root": progress.root}).decode(
                "import_progress", TypeAdapter(ImportProgress)
            )
            if not progress.complete:
                raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, "initial import is incomplete")
            tip = metadata.call(Operation.TIP).decode("tip", TypeAdapter(Tip))
            if tip.root != progress.root:
                raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, "initial import tip mismatch")
        nodes.projection.set_ref(reference=f"refs/cowtree/tips/{node.id}", commit=node.git_commit)
        write_record(path=self.staging / "workspace.json", record=self.config)
        sync_directory(path=self.staging)

    def status(self) -> ImportProgress | None:
        """Read durable server progress without waiting for the whole client import."""
        with Metadata(root=self.staging / "authority", binary=self.config.binary) as metadata:
            result = metadata.call(Operation.STATUS_IMPORT).decode(
                "import_status", TypeAdapter(ImportProgress | None)
            )
        return result
