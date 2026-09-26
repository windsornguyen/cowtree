Initial import
==============

.. note::

   Benchmark commands and receipts below describe the preceding implementation.
   Their harnesses remain in Git history. The native runtime is tested with
   ``cargo test --workspace --all-features``. Use ``benchmarks/README.rst`` for
   the native creation benchmark. Earlier timings do not qualify this refactor.

Workspace initialization captures one immutable source/cache node, then imports
its source manifest into the SQLite authority in bounded chunks. It creates no
per-file lease, view, upload, or proposal rows. This path is available only before
the authority has ever allocated a leaf. Ordinary source publication still uses
the checked candidate protocol.

The import identity is the hash of its canonical source manifest. Beginning the
same import returns its persisted progress; a different manifest is rejected.
Each chunk reads at most 64 source entries or 32 MiB, except that one larger file
uses its own chunk up to the configured object-size limit. The authority verifies
each hash and Git mode, refuses hard links and symlink parents, flushes each
object, flushes the containing directory once, then commits the cursor. The
immutable manifest and changing cursor occupy separate SQLite rows, so updating
progress does not rewrite the large manifest.

Epoch zero remains visible throughout import. ``finish_import`` verifies every
object and atomically records epoch one and the completion result. A crash can
leave extra pinned bytes; it cannot expose a partially imported snapshot or
advance acknowledged progress past durable objects. Collection protects unfinished
imports. A completed import record is an identity/progress receipt, not an
indefinite retention pin for its payloads.

Progress and recovery
---------------------

While initializing, observe acknowledged progress from another process::

    cowtree workspace --root /absolute/store import-status

The JSON value contains ``root``, ``completed``, ``total``, and ``complete``.
The count advances only after a durable chunk commit. It does not count files
while the initial immutable node is still being captured.

After interruption::

    cowtree workspace --root /absolute/store recover

Recovery holds the initializer lock, verifies the captured node, resumes from
the durable cursor, and publishes the completed workspace directory. Later
edits to the original checkout do not change that captured input. If capture
never completed, existing recovery removes the unacknowledged partial store.
Corrupt or changed captured bytes fail explicitly rather than being recaptured.

The Rust API exposes ``Workspace::import_status(root)`` and
``Workspace::recover_initialization(root)``. Standalone authority users have
``begin_import``, ``import_chunk``, ``status_import``, and ``finish_import``.
Stores must match the current declaration; opening an incompatible store fails
without schema or data migration. See `Declarative schema <schema.rst>`_. Import
failures include
``import_conflict``, ``import_not_ready``, and ``import_source_changed``.

Reproduction
------------

The experimental method, measured results, and limitations are reported in
`Workspace performance <performance.rst>`_. Generated measurements belong outside
the source tree; the scripts retain their own raw trials for analysis.

Benchmark commands
~~~~~~~~~~~~~~~~~~

The benchmark creates a new fixture and three stores, records time by protocol
operation, and independently compares source bytes against both the immutable
node and authority objects::

    cargo build --locked -p cowtree-metadata
    PYTHONPATH=src uv run python benchmarks/import_workspace.py \
        --root /absolute/new-benchmark \
        --binary target/debug/cowtree-metadata --files 128 --trials 3

Use ``--source /absolute/owned-clean-checkout`` for a large source-only project.
The benchmark never removes its evidence directory. ``cargo_workspace.py`` adds
derived target caches and real compiler/freshness checks; see
`Cargo workspaces <cargo-workspaces.rst>`_. Time is measured, not asserted in CI.

The regression suite includes real process kills before and after import
transactions, source corruption, namespace/mode validation, bounded chunks,
same-identity retries, and collection during import::

    cargo test --locked -p cowtree-metadata --all-features --test import --test import_crash
    cargo test -p cowtree-metadata --all-features import
    cargo test -p cowtree-cli --all-features failed_tree_flush

These tests qualify process interruption. A machine/storage power cut remains
a separate experiment. The required file and directory durability barriers are
preserved; the macOS duplicate-barrier change is measured separately in
`Durability benchmark <durability-benchmark.rst>`_.
