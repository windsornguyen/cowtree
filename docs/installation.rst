Filesystem installation
========================

``Installation`` prepares durable before/after images and an installation
record before replacing working files. The caller owns the root's lifetime,
excludes concurrent writers, and pins the source objects during preparation.
Root and journal must share a native CoW filesystem.

Each replay accepts the expected original entry or the already installed entry.
Different bytes cause an explicit conflict, preserving the file and journal.
Replay also checks the root directory's device and inode. Symlink parents,
parent traversal, control directories, hard links, and special files fail.

Replacements are atomic per file. The whole tree is not an atomic snapshot.
Readers with an open descriptor retain the old inode. The caller reconciles
activation or sync metadata only after every file is installed. It then calls
``finish``. An interrupted replay can resume, or roll back from before images,
provided nobody has edited the affected files in the meantime.

Source entries use Git's file/executable/symlink kinds. Installation gives
changed regular files fresh timestamps, even when the immutable
object was stored earlier. Unchanged paths retain their existing timestamps.
This preserves ordinary build invalidation inputs without promising cache validity.
Namespace transitions
remove only empty directories. Unmanaged files obstruct replacement instead
of being deleted. Derived directory metadata and cache validity belong to
the higher-level workspace policy.

Run ``COWTREE_EXPECT_SUPPORTED=1 cargo test -p cowtree-workspace installation``. These
tests cover interrupted calls and retained filesystem state, not power loss.

Coordinators can pass owned lock descriptors to ``CommandRunner`` and ``Metadata``.
Git projection preserves those descriptors in its child commands. A surviving
child then retains the operation's advisory lock if its parent is terminated.
Recovery must wait for that lock before removing files the child may still use.
``tests/test_process_ownership.py`` kills a controller and verifies this boundary.
