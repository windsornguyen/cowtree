Cargo cache comparison
======================

A warm Cowtree workspace completed creation and its first build in 0.675 seconds,
compared with 3.608 seconds for Git plus a warm shared compiler cache. Four warm
workspaces added 2.01 MiB of physical storage compared with 691.5 MiB for full copies.
These measurements cover the public ``cowtree-metadata`` crate, which produces
58 Cargo artifacts including SQLite, serde and procedural macros.

The pilot used Cowtree `df28962 <https://github.com/windsornguyen/cowtree/commit/df28962e1de66ea61a9a0150ada36975eeb95e4c>`_
on macOS 26.5 (25F71), APFS, a Mac15,9 with 16 logical CPUs and 128 GiB RAM.
Rust was 1.97.1, Python 3.10.21 and mbx 1.14.0. Builds used the development profile,
two jobs and disabled incremental compilation. The host also ran unrelated work.
Operating-system caches remained warm. Larger codebases and other filesystems
need their own measurements.

Build reuse
-----------

Three trials rotated the order of fresh Git worktrees, Git worktrees using a
shared mbx cache, and Cowtree forks inheriting a compiled ``target/`` directory.
Every build used the same commit, compiler, flags and dependency-cache path.
Timings exclude the independent file checks and executable checks.

.. list-table:: Warm-workspace timings, seconds
   :header-rows: 1

   * - Method
     - Creation median
     - Build median
     - Total median (range)
   * - Git, empty target
     - 0.073
     - 12.372
     - 12.446 (12.300-12.470)
   * - Git, shared warm mbx cache
     - 0.073
     - 3.535
     - 3.608 (3.582-3.729)
   * - Cowtree, inherited warm target
     - 0.550
     - 0.126
     - 0.675 (0.642-0.761)

Cowtree was 5.3 times faster than the warm mbx comparison and 18.4 times faster
than the empty-target comparison, including workspace creation. Cargo reported
all 58 artifacts fresh in every Cowtree trial, so it scheduled no compilation.
The first measured mbx trial avoided 66 compiler-wrapper invocations and had four
misses. Cargo's ``fresh=false`` in that comparison means it scheduled work whose
outputs mbx could then restore.

Priming the source target took 12.77 seconds. Importing that source and target
into Cowtree took another 1.20 seconds once per store. Priming the separate mbx
cache took 14.75 seconds. Those setup costs are excluded from the warm timings.
Preserving a valid target lets Cargo avoid both compilation and repeated
compiler-cache lookup and restoration.

Physical allocation
-------------------

Three paired trials created four warm worktrees from a seed containing 866 file
entries and 178,361,496 logical bytes. One arm copied the source and target files,
preserving their modes and modification times. The other used managed Cowtree
forks with ``derived_hardlinks=clone``.

Additional APFS allocation was 725,131,264 bytes for full copies and a median
2,109,440 bytes for Cowtree, a 99.7% reduction. Each arm used a separate
case-sensitive APFS sparse image. Container allocation was sampled after normal
detach/reattach until three readings agreed, counting shared extents once.
The seed and initial store were excluded from the four-worktree increment.
This comparison does not measure savings over mbx's deduplicated artifact store.

Correctness and limits
----------------------

Independent SHA-256, mode and modification-time checks preserved the seed and
untouched siblings. All nine executables passed version checks and real SQLite
initialization and reads. A source edit rebuilt one artifact and changed the
result. Concurrent builds in two leaves with different build-identity environment
values each rebuilt one artifact and reported their own value.

The tested revision passed 292 standalone/inline Python tests, 115 mounted tests
and 106 Rust tests. Nineteen Python cases were skipped. Passing mounted cases
cover process-kill recovery. Machine-reset and power-loss behavior remain unqualified.

Use real private cache directories. `Issue #34 <https://github.com/windsornguyen/cowtree/issues/34>`_
records a failing isolation case where ``target`` is a symlink to an external
writable cache and every leaf preserves that alias. Pinned submodule support is
requested in `issue #35 <https://github.com/windsornguyen/cowtree/issues/35>`_.
The build system remains responsible for cache validity and stopping writers
before capture. See the `consumer boundary <bsmr-workspaces.rst>`_.

Reproduce
---------

Run from the checkout containing these benchmark scripts. Install Rust 1.97.1
and mbx 1.14.0, then select the mbx executable with ``MBX_BIN``. Use absent output
directories and a dedicated dependency cache. The source is a public fixture::

    git clone https://github.com/windsornguyen/cowtree /tmp/cowtree-fixture
    git -C /tmp/cowtree-fixture switch --detach df28962e1de66ea61a9a0150ada36975eeb95e4c
    CARGO_HOME=/tmp/cowtree-cargo cargo +1.97.1 fetch --locked \
      --manifest-path /tmp/cowtree-fixture/Cargo.toml
    cargo +1.97.1 build --locked --release -p cowtree-metadata
    uv sync --locked --group test
    PYTHONPATH=src:. uv run --no-sync python -m benchmarks.cache \
      --source /tmp/cowtree-fixture --root /tmp/cowtree-cache \
      --cargo-home /tmp/cowtree-cargo --toolchain 1.97.1 \
      --binary "$PWD/target/release/cowtree-metadata" --mbx "$MBX_BIN" --trials 3
    PYTHONPATH=src:. uv run --no-sync python -m benchmarks.cache_space \
      --source /tmp/cowtree-cache/seed --root /tmp/cowtree-cache-space \
      --binary "$PWD/target/release/cowtree-metadata" --trials 3

The first command group retains per-build Cargo JSON, mbx statistics and
``report.json`` under its output root. The storage harness retains detached
images and allocation readings under its separate output root. Each image is
capped at 2 GiB and requires an 80 GiB host reserve. Use ``--trials 1`` for an
initial functional check. Raw reports and logs stay outside the checkout.
Run the mounted Cargo tests for source/environment invalidation and isolation.
