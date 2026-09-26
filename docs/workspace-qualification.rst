Managed workspace qualification
===============================

.. note::

   Benchmark commands and receipts below describe the preceding implementation.
   Their harnesses remain in Git history. The native runtime is tested with
   ``cargo test --workspace --all-features``. Use ``benchmarks/README.rst`` for
   the native creation benchmark. Earlier timings do not qualify this refactor.

``benchmarks/workspace_history.py`` runs seeded histories against real managed
Git worktrees and the SQLite authority. An independent model tracks file bytes,
executable modes, symlink text, published origins, private snapshots, and cache
payloads. It reads files directly with the Python standard library; it does not
use cowtree's scanner or installation planner to derive expected results.

Every action checks all live leaves, every explicitly retained snapshot, the
current warm tree, the SQLite source manifest, and active leaf inventory. Each
round creates a private checkpoint and descendant, captures two disjoint edits,
checks and publishes their union, preserves post-capture writes, synchronizes
views, and retires leaves in seeded order. Collection runs with live readers and
retained checkpoints. Reopening must preserve all modeled state.

The mutation schedule covers ordinary writes, deletion, executable files, and
symlinks; seeds choose paths, bytes, retirement order, and released checkpoints.
Every third round injects a lost response after the authority commits a batch.
Reopening and retrying must recover both receipts and the same checked warm tip.
This injection models uncertain acknowledgement, not sudden power loss. The
native Rust crash tests and mounted interruption tests cover additional boundaries.

Run
---

Build the required metadata executable, then run on APFS, Btrfs, or reflink-enabled
XFS. The script creates an owned temporary repository and removes it on exit.

.. code-block:: sh

   cargo build --locked -p cowtree-metadata --features fault-injection
   PYTHONPATH=src uv run python benchmarks/workspace_history.py \
       --seed 101 --rounds 6 --files 64 --cache-mib 2 \
       --output /tmp/cowtree-history-101.json
   uv run pytest mounted/test_workspace_history.py

``--source /path/to/local/repository`` clones a local Git source into the owned
fixture. The script adds its mutation files and explicit cache there, leaving the
original checkout unchanged. ``PYTHONPATH=src`` selects the checkout source instead
of an older installed wheel; the harness checks this identity. Without this option the source is deterministic
synthetic text. No network fetch is required. Cache payloads are deterministic
bytes; this harness checks their inheritance and isolation, not compiler hits.
``benchmarks/warm_cache.py`` separately measures actual make/compiler cache reuse.

Evidence
--------

Each JSON receipt records the command, seed, dimensions, elapsed time, action
counts, per-action verification times, and source/metadata-executable hashes.
Elapsed time includes full model verification after every action; it is not an
operation-throughput measurement.
The runtime source hash includes loaded cowtree modules and Rust metadata source,
including uncommitted edits. A changed runtime or executable during the trial
rejects the receipt. Fixture and final publication hashes identify modeled content.

``file_checks`` counts direct file-entry comparisons across live, retained, and
warm trees. Repeated checks of the same path count separately. Logical cache size
is an input dimension. These results do not measure unique physical disk blocks,
establish a space-savings percentage, or prove the absence of every bug. Use the
space benchmark for matched physical allocation measurements.

Local results
-------------

Three trials on September 16, 2026 completed 516 modeled actions and 307,082
direct file-entry comparisons. Each used six publication rounds and a 2 MiB
cache. Seeds 101 and 202 used synthetic sources; seed 303 cloned the local
cowtree subject at ``f97d7ccc8ff784689ac077af5e782296d0c752d2`` before adding the
mutation files. This is a modest repository qualification, not a large-project
scale result.

.. list-table:: Completed histories
   :header-rows: 1

   * - Seed
     - Initial source files
     - File comparisons
     - Elapsed seconds
   * - 101
     - 66
     - 79,185
     - 72.77
   * - 202
     - 66
     - 79,181
     - 54.46
   * - 303
     - 124
     - 148,716
     - 62.12

Combined histories completed 18 checked batches, 90 forks including 18 private
checkpoint forks, 87 retirements, 63 reclaimed nodes, and six recovered lost
acknowledgements. Nine deletions, nine executable changes, and nine symlink changes
were checked alongside ordinary and post-capture writes. All trials retained the
same runtime source and executable hashes from start to finish:

.. code-block:: text

   runtime: e2a15f5bca8117f8d6ee5a76fd2d308a3e6a2a3c0a6b65317e38b5ed41eab02e
   binary:  eae755b844e43bba46a01ad89e0b05ce4fec939926f4cd9365273df8e52ebbc4

The executable was a frozen copy of the verified production build, so concurrent
test builds could not replace it. Initial runs whose runtime or executable changed
were rejected and excluded. The JSON receipts preserve commands and per-action
counts; elapsed times include verification overhead and concurrent host activity.
