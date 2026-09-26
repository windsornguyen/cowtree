Collection and retention
========================

``Workspace::collect`` preserves the initial and current warm snapshots, live
leaf bases, explicit retention pins, pending captures and validation nodes,
and inputs named by incomplete client operations. Private parent history alone
does not retain a filesystem image. Retain a checkpoint explicitly when it
must remain forkable after its leaves are dropped.

Unreferenced images move atomically into quarantine before their refs and files
are removed. They cannot be newly retained while deletion is in progress.
Every normal workspace session finishes quarantine cleanup before admitting new
operations. Interrupted captures without an acknowledged node record are also
collectible. Active validation locks protect temporary check views and their logs.
Failed inactive check views are disposable and are reclaimed.

The last 128 local publication receipts retain their validation records and logs.
At least 64 recent check logs remain. SQLite maintenance independently preserves
its published, reserved, uploaded, captured, and client-origin object roots. These are retention
policies, not hard disk quotas. Unknown operation records stop image collection.
At most 256 snapshot directories, including the initial image and incomplete
captures, may be admitted before collection is required.
Quarantine records remain separate from the trees being deleted, so interrupted
recursive deletion does not erase the information needed to finish cleanup.

``Collection`` returns newly quarantined node identities, retired check leaf IDs,
and typed SQLite maintenance counts and file sizes. Cleanup resumed while opening
a session is not counted again. Required ``trash`` and ``receipts`` directories
are created during initialization; opening an incompatible layout fails without
adding missing directories.

The mounted tests cover process exits after recording quarantine, renaming a node,
removing its Git ref, and deleting part of its tree. They also overlap collection
with a blocked fork and an active check, and check receipt and log retention.
These process-exit tests do not simulate a power loss or faulty storage hardware.

Run ``cargo test -p cowtree-cli --all-features --tests`` on a native CoW mount.

Authoritative origin retention
------------------------------

A leaf can still need its original source bytes after the corresponding published
epoch has expired, even when it holds no lease. Every workspace session publishes
the union of leaf origins and acknowledged node origins into SQLite's durable
``client_pins`` table. Pin replacement validates new objects and commits additions
and removals atomically. Failed replacement keeps the previous pins; interruption
before acknowledgement leaves either the complete old or complete new set.

Sessions reconcile pins before admitting a caller and after a successful operation.
Collection refreshes them again after removing unreferenced nodes and before
object collection. Thus a released node's origins become reclaimable only after
its filesystem record is gone and no remaining client record needs those bytes.
Private source bytes that have never been uploaded remain owned by their snapshot
image; client pins describe authoritative origin objects, not a second payload copy.

Managed mutations must use ``Workspace`` methods, which hold the workspace session lock.
Direct metadata mutations during client filesystem operations are outside this
contract. Construction upload/proposal pins protect newly published bytes until
the successful client operation publishes its origin pins. On failure, old pins
remain conservative until recovery reconciles durable records.

Missing origin objects in an already damaged store are reported as errors; a
filesystem snapshot is not substituted for the authoritative object store.
