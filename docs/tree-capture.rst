Tree capture
============

The Python ``cowtree.trees`` module captures quiescent filesystem trees for
the local workspace adapter. ``PathPolicy`` selects disjoint relative prefixes
as derived or ephemeral. Other paths are source. Repository control directories
named ``.git`` or ``.cowtree`` are always excluded.

``scan_tree`` records kinds, modes, timestamps, and source content hashes.
``clone_tree`` creates an absent destination using the existing native CoW
primitive. It preserves derived bytes and timestamps without hashing cache
payload. Symlinks preserve their text and are not traversed. Empty directories
are retained. Hard-linked files and special files outside ephemeral subtrees
fail explicitly.

The caller must stop writers during capture. The before/after checks detect
observed changes but do not turn a file walk into an atomic filesystem snapshot.
The caller must also classify ignored files before calling this lower-level API.
Preserved cache bytes do not guarantee validity across paths or toolchains.
``populate_tree`` fills an already owned directory containing only its Git
control entry. It preserves that entry and leaves partial data for its caller's
operation journal to recover. ``clone_tree`` retains ownership of its own newly
created destination and removes it on failure.

This module does not register a Git worktree, publish a snapshot, or establish
durability. Those boundaries belong to the workspace adapter.

Run ``COWTREE_EXPECT_SUPPORTED=1 uv run pytest tests/test_trees.py`` on a
supported filesystem. Source and target must reside on a mount supporting
the native clone operation. A failed clone removes only its new destination.
