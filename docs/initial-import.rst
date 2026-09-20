Initial import
==============

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

The Python API exposes ``Workspace.import_status(root)`` and
``Workspace.recover_initialization(root)``. Standalone authority users have
``begin_import``, ``import_chunk``, ``status_import``, and ``finish_import``.
Schemas 1 through 3 migrate transactionally to schema 4. Import failures include
``import_conflict``, ``import_not_ready``, and ``import_source_changed``.

Reproduction
------------

Measured costs
~~~~~~~~~~~~~~

On one APFS host, three fresh 128-file stores (1 KiB per file) had these median
initialization times. The first four rows use the same debug build profile;
the last isolates the additional effect of an optimized release build.

=============================== =======
Implementation                  Seconds
=============================== =======
Per-file protocol               10.088
Bounded import chunks            2.191
Remove duplicate macOS barrier   1.230
Group complete-tree barriers     0.875
Grouped, release binary          0.746
=============================== =======

The old path made 391 protocol calls; the new path made six. This is primarily
a reduction in serialized transactions and storage barriers, not dynamic linking.
SQLite is compiled into the Rust executable. The release binary's dynamic
dependencies on this host were only system libraries, ``libSystem`` and
``libiconv``. Raw trials and executable hashes live in
``benchmarks/results/import-2026-09-19/small-import.json``. Active-host timings
are observations, not a subsecond service-level guarantee.
The final source rerun measured 0.807 seconds median; its complete trials are
in ``benchmarks/results/import-2026-09-19/final-import.json``.

A primed Ruff fixture with 17,696 source/cache entries imported in 38.2 seconds.
That run overlaps other test activity and differs from the earlier 15,811-entry
fixture; do not turn those two measurements into a controlled speedup claim.
Full Cargo results are in ``benchmarks/results/import-2026-09-19/cargo-ruff.json``.

Why large forks still take time
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

A quiet baseline profile of one Ruff fork took 25.3 seconds, including profiling
overhead. Four capture scans cost 12.4 seconds cumulatively, native clone calls
4.1 seconds, grouped durability 0.75 seconds, and seven metadata requests
0.37 seconds. Cumulative costs overlap. Three unprofiled forks measured
32.8, 16.0 and 20.6 seconds; each checked all 17,696 inherited entries against
the source with an independent byte/permission/timestamp oracle.

Managed forks now reuse the pinned manifest while copying metadata and retain
the final destination content check. Matched trials with the same starting live
leaf counts measured 8.99, 8.76 and 10.11 seconds, median 8.99 seconds. All 106,176
additional byte, full-permission and timestamp comparisons passed. Raw results
are in ``benchmarks/results/import-2026-09-19/forks.json``. Corrupt snapshots,
namespace races and writes with restored modification times remain refusal cases.

The remaining cost is per-file materialization and repeated validation. Faster
storage bandwidth or static linking alone cannot remove thousands of metadata
operations. Reusing retained snapshots avoids initial import; avoiding repeated
scans requires preserving a final comparison with the authoritative manifest.
A subsecond request for a large fresh directory would need prepared leaves or
a filesystem-level snapshot backend, with separate ownership and recovery tests.

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
    uv run pytest mounted/test_import.py mounted/test_import_review.py

These tests qualify process interruption. A machine/storage power cut remains
a separate experiment. The required file and directory durability barriers are
preserved; the macOS duplicate-barrier change is measured separately in
`Durability benchmark <durability-benchmark.rst>`_.
