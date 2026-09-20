Warm managed leaves
====================

``Leaves(workspace).fork(path, node=None)`` creates a managed Git worktree from
the current warm tip or a named private snapshot. It inherits that node's
eligible caches and retained origins. The destination must be absent, on the
workspace filesystem, with an existing parent, outside the store and every
existing worktree. The Git worktree remains locked against ordinary removal.

The operation records an empty directory's device and inode before exclusively
renaming it into place. It records the metadata allocation before registration.
The operation pins its source node and retains a lock while copying outside the
global workspace lock. Independent forks can therefore overlap. Completion
checks source bytes and the Git index before making the leaf record durable.

The pinned node already records the expected source hashes. Materialization checks
file metadata, link text, and source identity before and after native cloning.
The identity witness includes device, inode, and change time.
Completion hashes the entire destination source and compares it with that pinned
manifest before publishing Ready. This avoids repeated source hashing without
trusting an unchecked destination. Raw mutable-tree capture and cloning retain
their full content checks.

``recover`` skips live operation locks. It retires abandoned allocations and
removes only a matching owned directory and registration. A durable ready leaf
survives an interrupted operation cleanup. Uncertain ownership stops recovery
with the operation record retained. Direct mutation of the owned metadata store
or an initializing directory is outside this cooperative protocol.

Run ``cargo build --locked -p cowtree-metadata`` and ``uv run pytest mounted/test_fork.py``
on a native CoW mount. A barrier test proves overlapping clone phases without
turning host timing noise into a performance assertion.

Internal validation leaves carry an exact candidate identity. They cannot submit
source publications, and retirement waits for the request's validation lock.
This prevents cleanup from removing a directory still used by a surviving check.
A refused retirement records no deferred drop intent and does not block other
workspace sessions while the check continues.
