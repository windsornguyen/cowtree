"""Apply caller-selected local or published values under fresh path grants."""

from dataclasses import dataclass
from enum import Enum

from pydantic import TypeAdapter

from cowtree.durable import write_record
from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.install import Change, Installation, InstallRecord, local_path
from cowtree.leaves import Leaves
from cowtree.metadata_types import Grant, Operation
from cowtree.path_policy import check_aliases
from cowtree.views import ViewRecord, Views
from cowtree.workspace import Workspace
from cowtree.workspace_types import Leaf


class Choice(str, Enum):
    LOCAL = "local"
    PUBLISHED = "published"


@dataclass(frozen=True)
class Resolutions:
    workspace: Workspace

    def resolve(self, identity: int, choices: dict[str, Choice]) -> Leaf:
        """Keep selected bytes private while advancing their comparison origins.

        Abort a pending request first. A caller may write a manual merge then select
        ``local``; this operation never validates or publishes that merge.
        """
        if not choices:
            raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "resolution requires choices")
        if any(not isinstance(choice, Choice) for choice in choices.values()):
            raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "invalid resolution choice")
        with self.workspace.session() as metadata:
            leaves = Leaves(self.workspace)
            views = Views(self.workspace)
            leaf = leaves.read(identity=identity)
            if leaf.pending is not None:
                raise CowtreeError(
                    CowtreeErrorCode.INVALID_ARGUMENTS, "abort pending capture first"
                )
            if leaf.check_candidate is not None:
                raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "check views cannot resolve")
            current = views.current(leaf=leaf)
            for path in choices:
                local_path(root=leaf.path, relative=path)
            live = metadata.call(Operation.GRANTS).decode("grants", TypeAdapter(list[Grant]))
            check_aliases(paths=set(current) | set(choices) | {grant.path for grant in live})
            record = ViewRecord(
                kind="acquire",
                before=leaf,
                paths=sorted(choices),
                tokens=[grant.token for grant in live if grant.leaf == identity],
            )
            directory = views.begin(record=record)
            grants = metadata.call(
                Operation.ACQUIRE, {"leaf": identity, "paths": record.paths}
            ).decode("grants", TypeAdapter(list[Grant]))
            origins = dict(leaf.origins)
            held = dict(leaf.grants)
            changes: list[Change] = []
            for grant in grants:
                desired = (
                    current.get(grant.path) if choices[grant.path] is Choice.LOCAL else grant.origin
                )
                changes.append(
                    Change(path=grant.path, before=current.get(grant.path), after=desired)
                )
                if grant.origin is None:
                    origins.pop(grant.path, None)
                else:
                    origins[grant.path] = grant.origin
                held[grant.path] = grant.model_copy(update={"activated": True})
            updated = leaf.model_copy(update={"origins": origins, "grants": held})
            record = record.model_copy(update={"grants": grants, "after": updated})
            write_record(path=directory / "view.json", record=record)
            Installation.prepare(
                directory=directory / "installation",
                record=InstallRecord(root=leaf.path, changes=changes),
                objects=self.workspace.root / "authority/objects",
            )
            views.complete(metadata=metadata, directory=directory, record=record)
        return updated
