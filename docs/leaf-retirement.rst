Leaf retirement
================

``Lifecycle.drop`` retires authority and removes one managed Git worktree.
Private source changes require explicit ``force=True``. Resolve or abort a
pending publication first. Derived and ephemeral files belong to the disposable
leaf and are removed with it. Other leaves and immutable snapshots remain.

The drop intent is durable before authority or files are removed. Recovery
checks the recorded directory identity, retries registration removal, and
removes the leaf record only afterward. It refuses a reused pathname and keeps
the journal when ownership or cleanup cannot be established.

Run ``uv run pytest mounted/test_lifecycle.py`` on a native CoW filesystem.
