Private checkpoints
====================

``Workspace::seal(leaf, Retention::Manual)`` freezes the leaf's eligible files and caches,
records the previous node as its parent, and advances the managed Git view
without changing working bytes. Published per-path origins stay unchanged.
Forking that node inherits unpublished context but no reservation authority.

Checkpoints are retained by default. ``release`` removes an explicit retention
pin, and ``retain`` adds it again while the node exists. The limit is 128 manual
pins. Live leaves and pending operations are separate roots for collection.

The operation records its intended node identity before capture. Recovery
discards an incomplete, unacknowledged capture or completes the same durable
node and Git reference. Later working edits remain intact. The caller keeps
writers quiescent during the initial capture.

Workspace sessions reconcile abandoned local journals before admitting another
operation. Low-level inspection can use ``session(recover=False)`` to inspect a
blocked journal without applying it.

Run ``cargo test -p cowtree-cli --test managed private_checkpoints``.
