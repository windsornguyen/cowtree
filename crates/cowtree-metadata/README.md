# Local SQLite authority

`cowtree-metadata` is a Rust library and JSON command interface for a single-host
workspace metadata authority. It implements durable snapshots, fenced path
reservations, logical leaf edits, publication receipts, and explicit retention.
The Python `cowtree.workspace.Workspace` API connects this authority to real CoW
Git worktrees. It imports a pinned source, binds leaf directories, captures edits,
publishes single or batched changes, and installs or recovers acknowledged epochs.
See [managed workspaces](../../docs/workspaces.md) for the end-to-end API.

## Run

```sh
cargo run --locked -p cowtree-metadata --example publish
cargo test --locked --workspace --all-targets --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo fmt --all --check
```

The example creates a temporary authority, publishes one captured edit, preserves
a later dirty edit, repeats the commit, then reopens and checks the actual bytes.
No service or database setup is needed. SQLite is bundled through `rusqlite`.

The binary accepts one JSON object per input line and writes one response per line:

```sh
cargo run --locked -p cowtree-metadata -- /tmp/new-cowtree-authority <<'JSON'
{"op":"init"}
{"op":"create_leaf"}
{"op":"tip"}
{"op":"acquire","leaf":1,"paths":["src/main.rs"]}
JSON
```

`init` requires a new directory beneath an existing parent. Other commands require
an initialized store. Responses have `status: "ok"` with a typed `output`, or
`status: "error"` with stable `code`, `retry_action`, typed `details`, and a diagnostic
`message`. See [wire errors](../../docs/wire-errors.md). A request error does not terminate
the stream; callers must inspect each response. Rust callers receive the typed
`Error` enum. Input lines are limited to 128 MiB. `stage.data` is a JSON byte array;
the encoded request limit can be reached before the library's 64 MiB blob limit.

## Operations

| Rust operation / JSON `op` | Contract |
|---|---|
| `create` / `init`, `open` | Create a new authority or validate schema and required durability settings. Capture absolute paths so later `chdir` cannot redirect objects. |
| `create_workspace` / `init_workspace` | Publish a new authority and source configuration together without replacing an existing destination. |
| `import_tree` | Import a quiescent tracked source through native CoW with a durable bootstrap pin. |
| `create_leaf`, `leaves` | Allocate a never-reused identity or list active identities, including unbound interrupted adds. |
| `bind_tree`, `binding` | Verify and bind a physical directory; report its last acknowledged and pending epoch. |
| `capture_files` | Capture files, symlinks, modes, and deletions under current grants; record physical observation with each value. |
| `install`, `recover` | Journal a monotone installation and roll it forward before acknowledging; reject unrecorded edits and path collisions. |
| `tip`, `snapshot`, `read` | Return the current version/root, a retained manifest, or verified bytes at a retained version. Absence is explicit. |
| `acquire` | Reserve all requested paths atomically. An ancestor/descendant overlap excludes other leaves. Reacquiring an existing reservation keeps its token. |
| `activate` | Install the captured origin into the logical view once. Repeated activation preserves later edits. |
| `stage` | Pin, write, verify, and flush immutable bytes. A generation fences canceled or replaced uploads. |
| `edit` | Change or delete a logical entry under an activated current grant. New bytes require a ready upload owned by the leaf. |
| `view` | Return retained origins, values, and edit revisions from one metadata snapshot. |
| `release`, `revoke` | Release a grant or administratively revoke an exact path, preserving dirty data. Old tokens cannot publish or mutate a new grant. |
| `discard`, `discard_upload` | Explicitly discard local intent or an upload pin. Captured proposals remain immutable. |
| `drop_leaf` | Invalidate reservations, remove views/uploads, and abort pending requests. Committed receipts survive until their retention window expires. |
| `propose` | Freeze selected dirty values, origins, and tokens under a leaf/sequence request identity. |
| `prepare` | Overlay that capture onto the current tip, pin the candidate, and persist its manifest. Each attempt supersedes the previous attempt. |
| `commit` | Verify referenced bytes outside the writer transaction, then recheck exact candidate, tip, generations, and origins before atomically advancing tip and recording its receipt. |
| `prepare_batch`, `commit_batch` | Prepare disjoint full membership against an expected tip, then publish one epoch with all receipts atomically. |
| `resolve` | Explicitly choose a captured value per path from same-owner requests; freeze a replacement and retire source captures atomically. |
| `result` | Recover a retained receipt. Pending/aborted requests return no receipt; unknown or pruned identities return `RequestExpired`. |
| `abort` | Retire a pending request without permitting reuse of its sequence. |
| `retain`, `release_retention` | Pin/unpin an existing committed version. Manual pins have a configured count limit. |
| `maintain` | Prune history, collect unreferenced objects, reclaim free SQLite pages, and truncate the WAL. A blocked checkpoint returns `CheckpointBusy`. |

Use the JSON field names in `src/json_cli.rs`; library types are in `src/types.rs`.
`release` and `activate` take `grant`; `edit` takes `grant` and nullable `value`.
`propose` takes `input: {request: {leaf, sequence}, paths}`. `prepare`, `result`, and
`abort` take `request`; `commit` takes the complete returned `candidate`.

A successful commit updates a grant's captured origin. Reacquire the same path to
refresh its `Grant` handle before editing again; the token remains unchanged.
An old handle is rejected rather than silently refreshing its origin.

## Publication order

1. An immediate SQLite transaction captures path values, origins, and fencing
   tokens. The request sequence advances in this same transaction.
2. Preparation records its parent and attempt, constructs a full manifest outside
   the transaction, then records a construction pin before writing it.
3. Object publication writes a temporary file, flushes it, links its hash name
   without replacement, removes the temporary name, and flushes the directory.
   macOS also requests `F_FULLFSYNC`. Existing hash names are verified and flushed.
4. A second transaction marks the exact candidate ready. A superseded attempt
   cannot mark a newer candidate ready.
5. Commit reads the pinned parent and candidate and verifies every referenced byte
   before taking the writer transaction. It then revalidates exact identity,
   membership, readiness, tip, tokens, and origins and writes the epoch, updated
   origins, and receipts together. A canceled/superseded pin can cause an explicit
   read error; it cannot authorize stale publication. SQLite commits with `synchronous=FULL`.
6. Only after that commit does the caller receive the receipt. A retry with the
   same candidate returns that receipt without creating another version.

Disjoint proposals prepared against the same parent have one winner. The other
gets `TipChanged` and may call `prepare` again; the new candidate includes the
winner's unrelated edits. A changed touched-path origin requires a new capture.
Later local edits survive publication because commit advances origins without
replacing current values or revisions. A rename is a delete plus a put in one
proposal, with both paths reserved.

Origin equality is content equality. Returning A→B→A can make an older A-origin
capture admissible again if its lease generation is still current. Each accepted
publication still receives a distinct monotonically increasing version. This is
not version-based conflict detection or a claim of parity with every proposed
TLA+ transition.

## Retention and bounds

The defaults allow 64 active leaves, 100,000 logical paths/reservations, 256 pending
uploads and proposals each, 64 MiB per staged object, 16 recent snapshots, and 128
recent receipts. `Limits` are persisted at creation. At most `max_pending` manual
snapshot pins are admitted. Snapshot entries and retained views have separate
`max_paths` budgets.

Maintenance keeps the newest snapshot window, manual pins, bound installed and
pending targets, the initial imported source, and pending captured/prepared parents. It also protects lease origins, dirty views, upload hashes,
pending captured entries, and candidate manifests. Receipts prove publication but
do not keep bytes alive; pin a version when its bytes must survive later pruning.
A per-leaf sequence high-water mark prevents an expired request from executing
again; dropping a leaf does not allow reuse of its identity.

Pruning commits **before** collection starts. Collection takes a new immediate
transaction, reads all remaining pins, and unlinks only unprotected objects while
new pins are excluded. A rollback can never restore a reference whose bytes were
already removed. Interrupted collection can leave orphans for a later pass.
SQLite incremental vacuum is stepped to completion before a truncating checkpoint.

Admissions require maintenance before history exceeds these row budgets:

- Requests: `2 * retained_receipts + max_pending`.
- Epochs plus pending publication reservations:
  `retained_epochs + 4 * max_pending + 2 * retained_receipts`.

The 64 MiB default WAL threshold is an admission threshold, **not a hard disk
quota**. A transaction can overshoot it; cleanup remains available. Long external
SQLite readers can prevent truncation. `journal_size_limit` alone cannot impose a
WAL cap. Only this library may write the schema or owned object directory.
Object writers hold a shared advisory lock on the object directory from before
temporary-file creation through its removal and directory flush. Temporary
collection takes the exclusive lock, so it can remove crash leftovers even when
their hashes remain pinned. Writers release this lock before opening a ready-state
SQLite transaction; collection can hold SQLite while waiting without a lock cycle.
There is no claim of a strict total-object-space bound or a crash-safe disk quota.

## Support and evidence

Use a local filesystem with reliable SQLite locks and file/directory flushes, on
one Linux or macOS host. Network filesystems, hostile mutation of the owned store,
POSIX directory metadata, distributed consensus, and arbitrary concurrent raw
filesystem writers are outside this library's contract. Managed installation is
journaled per path, not an atomic whole-tree switch; editors and readers requiring
a coherent tree must be quiescent until its acknowledgement or recovery. The source,
authority, and worktrees must share a native-CoW filesystem. Namespace aliases that
the authority filesystem cannot distinguish are rejected before publication and installation.
Hard-linked regular files are rejected during import or capture; materialized files
use separate CoW inodes.
Resource paths are normalized UTF-8, case-sensitive paths with no empty, dot,
dot-dot, NUL, or case-insensitive `.git` component. Entries are files, executable files, or
symlinks. Empty directories and arbitrary Unix metadata are not represented.

SQLite WAL requires same-host coordination. The library checks WAL/FULL settings,
foreign keys, incremental vacuum, and a SQLite version at least 3.51.3 for the
upstream WAL-reset fix. See [SQLite WAL](https://sqlite.org/wal.html) and
[SQLite durability pragmas](https://sqlite.org/pragma.html#pragma_synchronous).

Tests exercise real temporary stores, independent connections and CLI processes,
process exits before/after publication, collection interleavings, post-capture
edits, stale generations, exact retries, and seeded state-machine histories.
`fault-injection` is an explicit test feature; production builds ignore its crash
and pause environment variables. Exiting a process is not a power-cut test.
Full snapshot reads and collection retain the writer lock; commit payload hashing
runs outside it. See [qualification](../../docs/qualification.md) and
[recorded results](../../docs/qualification-results.md) for measured sizes,
latencies, contention, and limitations. Schemas 1 and 2 migrate transactionally to
schema 3; older views begin without physical-capture provenance.
