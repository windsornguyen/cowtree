Agent filesystem
================

Design reference for local and future native filesystems. The implemented local
CLI and Python contract live in `Managed workspaces <managed-workspaces.rst>`_.
The interface sketches and native/distributed roadmap below are design targets;
they do not extend that supported API. Primary safety model: ``specs/Core.tla``.
Service model: ``specs/EpochLog.tla``. See `Verification <verification.rst>`_
for the checked persistence model and implementation traces.

The goal is an immutable snapshot history with mutable, single-writer leaves.
A leaf is an agent's private filesystem view. Git supplies history and review;
a coordinator owns reservations and publication. Filesystem isolation alone
does not isolate processes, ports, databases, credentials, or environment.

Identities and ownership
------------------------

A node has two identities: its content root identifies bytes and metadata;
its history identity includes the root, parents, and node metadata. Equal
content after an edit and its reversal must produce distinct history nodes.
Hash-based identity assumes collision resistance and verified object contents.

A leaf records a retained base node, installed origins, local edits, and a
writer identity. Sealing creates an immutable private node; it does not publish.
A workspace has one authoritative tip. Each published node is a child of the
previous tip. A batch advances the workspace version once.

The coordinator owns tip, lease generations, proposal identities, and results.
These records share one transaction boundary. A leaf owns local installation
and edits. A storage backend owns durable objects and their retention.
A client must resolve an uncertain publication by its existing identity.

Path classes
------------

Source
  Authoritative files, including dependency lockfiles. Source changes may be
  published. Lockfiles need a format-aware merge or explicit resolution;
  regenerating them may change dependency decisions.

Derived
  Build caches and dependencies such as ``target/`` and ``node_modules/``.
  A whole-tree fork can inherit them within a lineage. They never travel
  through source publication. A warm-tip build can refresh the tip lineage.

Ephemeral
  Sockets, logs, credentials, and local environment files. Never sealed or
  published. Classification must be explicit enough to avoid treating every
  ignored file as a safe cache.

Preserving bytes and timestamps preserves those inputs to a build tool.
Cache validity can also depend on absolute paths, toolchains, environment,
and machine state; a fork cannot promise a cache hit. The tracked-only ``add``
command excludes caches. Managed ``workspace fork`` inherits classified caches;
the `build-cache experiment <warm-cache.rst>`_ measures actual reuse.

Namespace contract
------------------

Reservations conflict on prefix overlap. Renaming reserves both source and
destination names and affected namespace membership. Creating and deleting
children must coordinate with parent-directory operations. The flat symbolic
keys in the current models do not implement these rules.

Sealing must reject unsupported hard links and special files explicitly.
Symlinks retain their link text; traversal and namespace normalization need
a defined policy. Sync must preserve dirty and submitted paths. Replacing a
clean file leaves existing descriptors and mappings attached to the old inode;
the runtime must treat sync as a visible event and reopen as needed.

Proposed operations
-------------------

These names define responsibilities, not released Python signatures. The
`workspace protocol <workspace-protocol.rst>`_ specifies the reservation,
activation, proposal, and result boundaries in more detail.

Fork(node) -> leaf
  Pin a retained immutable node before materialization and create a fresh
  writer identity. Forking a live leaf first requires a coherent seal.

Write(leaf, path, content)
  Change the leaf's private view. Unreserved edits are allowed locally but
  cannot bypass fencing at publication.

Acquire(leaf, resources) -> reservations
  Atomically reserve all resources or reject the request. Return fresh
  monotone generations and pinned tip content. Install into generation-specific
  staging, then activate only if those generations are still current.
  Preserve dirty data for resolution or require an explicit discard.

Sync(leaf)
  Install current content only on paths with neither local edits nor an
  unresolved submitted proposal. A later edit stays above its captured
  proposal until the result is reconciled.

Seal(leaf) -> node
  Freeze a coherent view and retain its source and eligible derived objects.
  A nominal single writer is insufficient if that writer's build processes
  are still mutating files; quiesce them or use a coherent backend snapshot.

Publish(leaf, proposal_id) -> result
  Freeze delta, origins, tokens, and edit generation. Build against the current
  tip, persist referenced objects, validate the exact candidate identity,
  then atomically check expected tip and tokens while recording the result.
  Rebuilding a candidate invalidates its validation. Acknowledgement must
  preserve edits made after proposal capture.

Release(leaf, reservations)
  Release only the matching identity and generations. Retained edits become
  stale. The abstract EpochLog model only releases clean, unsubmitted paths;
  the richer workspace contract must reconcile that restriction explicitly.

Expire(reservations)
  Revoke authority without relying on the old client receiving a notification.
  New grants use fresh generations. Timeouts decide when to revoke; publication
  checks current authority independently of clocks.

Discard(leaf, paths)
  Explicitly abandon local edits. With a proposal pending, discard only the
  later edits and restore the captured proposal value and origin. Discarding
  the proposal itself requires a separate, resolved cancellation contract.

Drop(leaf)
  Invalidate the writer identity and reservations, resolve or cancel pending
  proposals, and release pins. No unpublished work is implicitly published.
  Reusing a slot must not resurrect requests from an earlier incarnation.

GC()
  Reclaim only objects unreachable from retained history, active leaves,
  pending proposals, candidates, readers, and in-progress forks. Coordinate
  reclamation with new pins and crash recovery.

Mount(node | leaf)
  Expose a node read-only or a leaf to its exclusive writer. Sandbox side
  effects remain a separate runtime responsibility.

Publication and conflicts
-------------------------

A batch applies deltas to the current tip; untouched paths come from that tip,
never from an older leaf snapshot. An older base version is acceptable when
its relevant origins and authority remain valid.

Check pairwise disjoint write sets explicitly. Exclusive leases separate
different holders, but do not prevent overlapping proposals from one holder.
EpochLog currently restricts each leaf to one pending proposal; production
queues must enforce their own equivalent condition.

Every tip advance, lease change, and result lookup needs the coordinator's
ordered state. Reading an already-known retained immutable version does not
need to discover the latest tip. The models do not prove serializable
transactions, read-set validation, multi-region sequencing, or availability.

Path disjointness does not imply semantic compatibility. Validate the exact
candidate, including its parent, before committing. A change to one file may
invalidate another file's callers even when their leases do not overlap.

A contested source path uses explicit merge inputs and structured conflicts.
A cached resolution key includes base, ours, theirs, path/mode metadata,
merge policy, driver version, configuration, and any additional inputs.
Content hashes alone do not make an arbitrary resolver deterministic.

Git projection
--------------

A node's source tree projects into Git objects; derived and ephemeral paths
are excluded. Node refs use ``refs/cowtree/nodes/<id>``; workspace tips use
``refs/cowtree/tips/<workspace>``. These refs are proposed, not created today.

Use Git's merge machinery with a defined base and structured conflict output.
Conflicts include rename/delete, directory/file, modes, and binary add/add;
conflict markers alone are insufficient. Unresolved conflicts remain explicit
data and block ordinary publication.

Materialize a merge from the tip lineage, then apply changed source paths.
Inherit only that lineage's eligible caches. Changes receive appropriate
metadata so the build tool can invalidate them.

Backends and lock scope
-----------------------

The implementation calls APFS ``clonefile(2)`` or Linux ``FICLONE``
for each selected regular file. These walks scale with file count. The engine
probes actual clone support and fails closed; no copy fallback exists.

Managed workspaces implement classified whole-tree walks. Btrfs subvolume
snapshots, ZFS dataset clones, and a native epoch backend remain future interfaces.
Their cost and consistency contracts
require separate qualification. No fixed latency or 99% physical-space saving
follows from the symbolic model.

The tracked-only repository lock covers registration, cloning, index validation,
and rollback. Managed forks release their workspace lock while cloning an
immutable pinned node. An operation record owns the initializing target and
pins the source against collection. A per-target index lock alone would not
protect a mutable source being removed.

The concurrency acceptance test must use barriers to prove two independent
clones overlap, then race source removal, target removal, same-target adds,
and failure cleanup. Each result must be complete or fail with owned-state
cleanup. Timing ratios alone are insufficient.

Local contract and native roadmap
---------------------------------

Whole-tree forks
  Managed forks provide explicit classification, cache inheritance, byte and
  timestamp tests, and a real build-reuse experiment. Tracked-only mode remains
  available. Physical-space measurements are a separate benchmark.

Local metadata
  A configured store outside checkouts keeps the SQLite authority and durable
  filesystem journals. Fork, seal, log, collection, and recovery have real-file
  tests. Client origin pins extend retention across authority history pruning.

Local coordination
  Prefix reservations, staged installation, sync, frozen captures, checked
  candidates, atomic batches, and explicit resolution are implemented. Selected
  model traces replay through these APIs and compare each observable post-state.

Remote backend
  A native epoch adapter, remote lease service, cross-machine leaves, and warm
  tips. Validate the adapter against its repository before claiming support.
  Private infrastructure mappings are not implementation evidence in cowtree.

Acceptance and verification
---------------------------

`Verification <verification.rst>`_ distinguishes model checks, implementation
tests, storage qualification, and proofs. Every new operation needs an
observable contract and failure tests. PublicationRecovery models abstract
durable-object, restart, and collection ordering. Namespace operations and physical
sharing remain runtime qualification obligations. Bounded model checks are not
a refinement proof of the complete implementation.
