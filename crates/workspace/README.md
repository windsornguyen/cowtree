# Managed workspaces

This crate owns warm worktrees, checkpoints, checked publication, and recovery on
macOS and Linux. It uses native tree operations from `libcowtree`, the in-process
SQLite authority from `cowtree-metadata`, and Git subprocesses.

```text
CLI -> Workspace -> filesystem journals -> native clones
                 -> Store              -> SQLite + immutable objects
                 -> Repository         -> git
                 -> native supervisor  -> validation command
```

| Surface | Contract |
| --- | --- |
| `Workspace`, `CreateRequest` | Admit a source and reopen an identity-checked store. |
| `Leaf`, `Node`, `Policy` | Record ownership, comparison origins, and source/cache selection. |
| `CheckRequest`, `Validation` | Check an exact candidate in an isolated warm worktree. |
| `Choice`, `DropPolicy`, `Retention` | Express caller consent and retention explicitly. |
| `Collection` | Report retired client images separately from authority reclamation. |

Every filesystem transition takes the workspace lock and reconciles durable
intents in dependency order. Fork population releases that lock while an operation
lock pins its input. Git children inherit held locks. Validation runs under a
separate native supervisor so coordinator death cannot release ownership early.

A session holds one `Store` connection. Metadata subprocesses are not part of a
managed operation. Capture retains that session across checkpoint creation,
reservation, and submission. Source-only Git projections use private indexes and
conditional ref updates through the shared [`Git client`](../git/README.md).
The configured common directory is resolved once and reused by managed operations.

Checkpoint capture writes an immutable image and record. Publication verifies
the image after Git filters have run, then writes its ref once. Journal completion
acknowledges the leaf. Recovery uses the same verified publication operation. Private worktrees
remain detached. A validation command that changes source cannot publish its candidate.

```sh
cargo test -p cowtree-workspace
cargo test -p cowtree-cli --all-features --tests
```

Set `COWTREE_EXPECT_SUPPORTED=1` on a qualified clone filesystem. The default
build has no crash hooks. The `fault-injection` feature enables explicit test
boundaries only.

See [the managed contract](../../docs/managed-workspaces.rst),
[protocol verification](../../docs/verification.rst), and
[Git worktree semantics](https://git-scm.com/docs/git-worktree).
