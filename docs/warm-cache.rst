Warm build cache qualification
==============================

The fixture builds two C translation units and an executable with ``make`` and
``cc``. Its ``build/`` directory is an explicit derived-cache prefix. After the
source checkout has been built, an ordinary Git worktree and a Cowtree fork are
created from the same committed source.

The ordinary worktree starts without build outputs and compiles both source files.
The Cowtree fork inherits the objects and executable with their modification times.
A successful warm build invokes neither the compiler nor linker and leaves every
artifact modification time unchanged. The fixture then changes ``value.c`` in each
worktree. Both must rebuild that object, relink, and print ``43`` instead of ``42``;
``main.o`` and the original checkout's build outputs must remain unchanged.

Reproduction
------------

Use a native copy-on-write filesystem with Git, make, and a C compiler installed::

    cargo build -p cowtree-metadata --bin cowtree-metadata
    PYTHONPATH=src uv run python -m benchmarks.warm_cache \
        --root /tmp/cowtree-warm-build-run \
        --binary target/debug/cowtree-metadata \
        --trials 3 --output /tmp/cowtree-warm-build.json

``--root`` must be absent. Every trial retains its seed checkout and workspace for
inspection and removes its two test worktrees. The JSON report includes toolchain
versions, workspace initialization time, worktree creation time, build time,
compiler/linker invocation counts, and executable results. An assertion failure
exits unsuccessfully without producing a success report.

Timing uses a monotonic clock. The source-edit phase waits for a filesystem timestamp
tick outside the timed build, allowing make implementations with whole-second
mtime comparisons. Workspace initialization is measured separately because its
cost is paid once before forks. Build timing excludes worktree creation and
executable verification.

Local measurements
------------------

Three runs on macOS 26.5.2 arm64 with Apple Clang 21.0.0 and GNU Make 3.81
produced these median times on the frozen implementation working tree,
including client-origin pins and initialization recovery:

.. list-table:: Three-trial medians, milliseconds
   :header-rows: 1

   * - Operation
     - Ordinary Git worktree
     - Cowtree warm fork
   * - Create worktree
     - 22.63
     - 262.81
   * - Initial build
     - 101.52
     - 8.78
   * - Create plus initial build
     - 126.28
     - 271.52
   * - Build after changing one source file
     - 65.45
     - 62.17

All three ordinary builds compiled two objects and linked once. All three warm
builds compiled and linked zero times, preserving artifact modification times.
Both variants then rebuilt one object and linked once after the source edit,
producing the expected changed output. Initial workspace creation took a median
1161.11 ms, separately from the fork timings above.

The copied production binary and 53 Python/Rust/build-input hashes remained
unchanged throughout these three trials. All three stores used the database
layout from that measured revision.
CLI files were excluded because this benchmark does not import them.

For this tiny fixture, Cowtree's worktree creation cost exceeds the avoided build
work: creating and building an ordinary worktree finishes sooner overall. The
experiment proves cache reuse and correct invalidation, not an end-to-end speedup.

Scope
-----

This fixture qualifies a relative-path, timestamp-based make build. It does not
establish cache reuse for every toolchain. Absolute paths, environment changes,
compiler versions, dependency databases, and generated configuration can invalidate
other build caches. The workload measures reuse and rebuild correctness; it does
not measure physical disk savings or establish a general build speedup.

Run ``PYTHONPATH=src uv run pytest mounted/test_cache_build.py`` to check the same
invariants without interpreting short benchmark timings as a performance promise.
