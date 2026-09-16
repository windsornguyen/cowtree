# Managed workspaces

`cowtree.workspace.Workspace` connects native copy-on-write Git worktrees to the
Rust SQLite authority. The existing `cowtree add/list/remove/doctor` API remains
available for independent Git worktrees.

The metadata executable is required. Build it with `cargo build --release -p
cowtree-metadata`, then pass its path explicitly. Source checkout, authority, and
worktrees must use a filesystem supporting native reflinks: APFS, Btrfs, or XFS
configured with reflinks. Unsupported filesystems fail; no byte-copy fallback runs.

## Publish files

```python
from pathlib import Path

from cowtree.workspace import Workspace
from cowtree.workspace_types import RequestId

workspace = Workspace.create(
    source=Path("/work/project"),
    authority=Path("/work/project-authority"),
    binary=Path("/tools/cowtree-metadata"),
)
writer = workspace.add(Path("/work/writer"), branch="agent/change")
reader = workspace.add(Path("/work/reader"))

grant = workspace.activate(workspace.acquire(writer, ["src/example.py"])[0])
(writer.path / "src/example.py").write_text("print('published')\n")
workspace.capture([grant])
receipt = workspace.publish(RequestId(writer.leaf, 1), ["src/example.py"])
workspace.sync(reader, version=receipt.version)
```

Creation imports only Git-tracked files from a clean source checkout. It records
the source commit and retains that initial epoch so new leaves can clone the
source and then install later publications. Moving the source to another Git
commit requires a new workspace.

A publication commits metadata and immutable content. `sync` separately installs
an epoch in a bound worktree. Git HEAD and index are preserved, so published edits
appear as ordinary Git changes. Symlink text and executable modes are preserved;
untracked paths are preserved and a conflicting untracked path blocks installation.
Changed regular files receive a fresh modification time, so restoring an older
content object does not silently reuse newer timestamp-based build outputs.

## Operations

| Operation | Contract |
| --- | --- |
| `create(source, authority, binary)` | Import a clean source and persist its initialization record. |
| `open(source, authority, binary)` | Reopen the recorded workspace; resume a recorded incomplete import. |
| `add(path, branch=None)` | Create a CoW Git worktree, bind an identity, and install the current tip. |
| `leaves()` / `binding(leaf_id)` | Inspect active identities and their persisted installation state. |
| `acquire(leaf, paths)` | Reserve exact paths; unchanged paths can be leased across epochs. |
| `activate(grant)` | Activate the supplied current reservation. |
| `capture(grants)` | Read current file bytes, modes, symlinks, and deletions into logical views. |
| `propose(request, paths)` | Freeze selected dirty views under a request identity. |
| `prepare(request)` / `commit_candidate(candidate)` | Construct and commit a publication with exact candidate identity. |
| `publish(request, paths)` | Propose, prepare, and commit already captured views. |
| `prepare_batch(requests, expected_tip)` | Prepare complete disjoint membership against an explicit tip. |
| `commit_batch(candidate)` | Commit every batch member in one epoch and return all receipts. |
| `resolve(request, sources, choices)` | Replace same-owner captures with explicit per-path selections. |
| `result(request)` | Look up a durable receipt after an uncertain commit response. |
| `sync(leaf, version=None)` | Install the selected epoch, defaulting to the current tip. |
| `recover(leaf)` | Complete a journaled interrupted installation. |
| `release(grant)` | Release the supplied current reservation. |
| `remove(leaf, force=False)` | Apply Git's removal checks, then retire the metadata identity. |

Request sequences start at one for each leaf. Persist the request identity before
sending a publication. After losing a commit response, query `result(request)`;
retry the exact candidate if it has not committed. A tip change requires preparing
the same frozen proposal again. Expired identities never execute a new publication.

Publication changes reservation origins, so reacquire the same owned paths to
obtain current grants before a subsequent capture or release. Errors preserve
`MetadataError.code`, `retry_action`, and structured `details`; messages are for
people. See [wire errors](wire-errors.md) for the native protocol.

## Atomic batches

After capturing and proposing disjoint changes from two managed writers, prepare
both request identities against one explicit tip:

```python
requests = [RequestId(first.leaf, 1), RequestId(second.leaf, 1)]
candidate = workspace.prepare_batch(requests, expected_tip=workspace.tip().version)
batch = workspace.commit_batch(candidate)
workspace.sync(reader, version=batch.receipts[0].version)
```

Every returned receipt names the same epoch and content root. Commit takes the
complete prepared membership; members cannot be committed separately. A tip change
requires preparing the batch again. An uncertain response can be resolved by
retrying the exact candidate or querying each member with `result`.

Conflicting paths fail with `batch_conflict`; no member publishes. For overlapping
captures belonging to one writer, explicitly select a source capture for every
path and assign a new request sequence:

```python
first_capture = RequestId(writer.leaf, 1)
second_capture = RequestId(writer.leaf, 2)
resolved = RequestId(writer.leaf, 3)
workspace.resolve(
    request=resolved,
    sources=[first_capture, second_capture],
    choices={"src/example.py": first_capture},
)
receipt = workspace.commit_candidate(workspace.prepare(resolved))
```

The choices must cover every path present in the selected captures. Resolution
retires those source captures atomically and creates one replacement proposal.
It selects immutable captured values, does not edit files, and preserves later
local edits. Cross-owner captures require their owners to resolve their work;
the resolver does not choose winners or merge source text automatically.

## Local edits and conflicts

Stop editors while capturing or installing. SQLite fencing controls cooperative
API calls; it cannot prevent an arbitrary process from writing a pathname.
Installation changes files individually and is not an atomic whole-tree switch.
Acknowledged installation versions only advance; an older retained epoch can be
read without rolling an existing worktree backwards. Capture records each file
independently; proposals and commits provide the atomic publication boundary.
Readers requiring a consistent view must also be quiescent during installation.

Installation refuses unrecorded local edits. To preserve a later edit while
publishing an earlier capture, use separate proposal and commit steps:

1. Capture edit A, call `propose`, then `prepare`.
2. Make edit B and capture it under the current grant.
3. Commit A's candidate. Its receipt publishes A; the retained local view contains B.
4. Sync the writer. The recorded dirty B is preserved while its origin advances to A.

Publishing neither commits nor discards Git changes. `remove` fails on dirty Git
worktrees unless `force=True` is explicitly supplied.

Managed removal shares the authority's filesystem lock with native installation,
recovery, capture, and binding. Removal waits for an active installer, then checks
the acknowledged binding before invoking Git. An interrupted installation must be
recovered before removal. Git failures become `MetadataError` without discarding
their error codes, and the lock is released on failure.

## Recovery

Open the same authority and use `leaves()` and `binding(id)` to inspect persisted
state. A binding reports its last acknowledged `version` and an optional `pending`
target. Call `recover(leaf)` for a pending installation before other mutations or
removal. Recovery verifies the recorded directory identity and accepts only the
recorded old/new contents; unexpected files fail closed.

An interrupted add can leave an unbound identity and a Git worktree. Inspect
`leaves()` and Git's worktree list, then finish registration with
`workspace.bind(WorkspaceLeaf(leaf=id, path=path))` and `sync`. Binding verifies the
worktree matches the original imported snapshot. It does not adopt arbitrary data.

Initialization builds the SQLite authority and its pinned-source record in a
private sibling directory, then publishes them together with a no-replace directory
rename. Before publication, a crash leaves the final authority absent and `create`
can be retried. After publication, `open` resumes the recorded import. An existing
destination is never overwritten. Concurrent openers re-read the initialization
record under its Git lock and reuse a completed import, so a stale pending record
does not re-enter filesystem initialization. Forced termination can leave an
inspectable `.cowtree-init-*` staging directory; it is not a published workspace.

## Validation

Build fault injection explicitly for the process-crash tests:

```sh
cargo build -p cowtree-metadata --features fault-injection
COWTREE_METADATA_BINARY="$PWD/target/debug/cowtree-metadata" \
  uv run pytest -q tests/test_workspace.py tests/test_metadata_client.py
```

Native integration tests require `COWTREE_METADATA_BINARY`; an explicitly supplied
missing binary is a failure. Run on a supported filesystem. The tests cover real
Git worktrees, publication and reopening, new leaves at later epochs, mode and
symlink preservation, deletion, dirty conflicts, atomic batches, explicit resolution,
post-capture edits, and interrupted authority publication, imports, and installations. These process-crash tests do not
simulate sudden power loss.
