Read-only snapshot namespaces
=============================

A pinned dependency must not become writable because a caller bypasses the
filesystem coordinator. The Rust metadata authority records read-only scope
inside each protected entry's kind and checks it before allocating path grants.

The wire representation is explicit::

    {
      "object": "<sha256 of file bytes>",
      "kind": {
        "read_only": {
          "file_kind": "file",
          "scope": "vendor/dependency"
        }
      }
    }

The scope must contain the entry's resource path. A scope can cover a file or
an entire directory namespace. Acquiring the scope, one of its descendants, or
an ancestor that could replace it returns ``read_only_path``. A failed batch
allocates no grants or tokens, including for its otherwise writable paths.

Only the initial import can establish this policy. ``edit`` cannot turn a
writable grant into a read-only entry. Existing ``file``, ``executable``, and
``symlink`` values keep their wire representation and content identities.

Older readers reject the new tagged kind. An extra optional JSON field would
be unsafe because an older reader could ignore it and grant write access.
Invalid scope declarations return ``invalid_read_only_scope`` before import
progress is recorded.

These rules govern the authority's logical namespace. They do not change OS
file permissions or prevent direct writes to a private working directory.
The filesystem coordinator must still compare captured bytes with the pinned
source. Submodule materialization is a separate layer.

Python ``Entry.file_kind`` exposes the physical kind while full record equality
retains the read-only policy. Installation journals preserve that policy across
replay. File-image checks compare bytes and mode, not logical access metadata.

Run the admission and compatibility invariants::

    cargo test -p cowtree-metadata --test readonly
