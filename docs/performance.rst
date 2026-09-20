Workspace performance
=====================

Scope
-----

Cowtree avoids copying file payloads when a native copy-on-write filesystem can
share them. A usable workspace still needs private names and metadata, a Git
registration, validated source contents, and a durable ready record. Measure the
acknowledged workspace operation separately from cloning, compilation, and
independent correctness checks. The measurements below do not establish a
subsecond fresh fork for a large Rust project.

Methods
-------

The macOS measurements used an Apple M5 Max (Mac17,6) with 18 logical CPUs and
48 GiB memory, macOS 26.5.2 build 25F84, and an APFS Data volume with approximately
175 GiB free. Python was 3.10.20. Cowtree's metadata service used Rust 1.97.1;
the Ruff build fixture used Rust 1.98.0. The comparison distinguishes debug and
release metadata executables. Runs shared an interactive development host; some
native-clone measurements overlapped cleanup of owned leaves. Operating-system
caches were warm and were not flushed. Background activity was not controlled.

The large fixture was Ruff commit
``5bb508f59481c1a02d5c7ff8568b9ffd8a763eef`` with a built Cargo target directory.
Its frozen node contained 17,695 regular files, one symlink, and 1,625 nested
directories, with 2,699,414,721 logical file bytes. Cargo outputs were explicitly
derived state and hard-link cloning was enabled. Source and cache files were
quiescent before capture. The measurements preserve full permission modes and
modification times; source manifests still distinguish Git's executable bit.

The Cargo fixture used the development profile with optimization level zero,
line-table debug information, no link-time optimization, and incremental
compilation disabled. Its primed lane used the same private Cargo-home path for
source and leaf builds. A separate lane with relocated registry paths cannot be
compared as if it started with equivalent compiler-cache identities. Cargo's
reported fresh/compiled artifacts and functional smoke output establish reuse;
the presence of a cloned target directory alone does not.

Each timing series reports three observations and their median unless explicitly
identified as a single run. Medians and observed ranges are descriptive; no
latency-tail estimates or service-level guarantees are inferred from these small
samples. Repeated forks accumulate live leaves unless the harness explicitly
retires them, so comparisons must also record the initial leaf count.

Import
------

``benchmarks/import_workspace.py`` creates a new store for each trial and times
initial capture through publication of the completed workspace. Its independent
oracle then compares each input with the private node, the authority object, and
the original source. Oracle time is outside the operation timer.

For 128 generated files, replacing per-file publication with resumable bounded
chunks reduced protocol calls from 391 to six. The debug-build comparison was:

.. list-table:: Matched small import, seconds
   :header-rows: 1

   * - Implementation
     - Median
     - Observed range
   * - Per-file publication
     - 10.088
     - 9.308 to 10.244
   * - Bounded import, original flush path
     - 2.191
     - 2.024 to 2.469
   * - Bounded import, one full flush per object
     - 1.230
     - 1.144 to 1.852
   * - Bounded import, grouped macOS barriers
     - 0.875
     - 0.872 to 1.232

A later release-build run of grouped import measured 0.807 seconds, with an
observed range of 0.789 to 0.817 seconds. Keep this separate from the debug-build
comparison. Each three-trial series checked 1,152 file comparisons. The chunk
cursor becomes visible only after its object durability boundary; reducing
transaction count does not remove that boundary.

The user-reported 1,537.9-second Ruff import motivated this work but was not
repeated as a matched baseline. The primed source-plus-Cargo-cache fixture took
38.214 seconds in one grouped-import run. These observations use different fixture
states and mechanisms; they do not support a claimed ratio.

Fresh forks
-----------

The managed-fork timer begins before ``Leaves.fork`` and stops after its durable
ready record and Git state are established. The independent oracle then checks
all inherited files and the unchanged original source, including bytes, modes,
modification times, and symlink destinations.

The final checked-in benchmark kept six existing leaves throughout the series,
retiring each completed trial leaf before the next trial. Fresh forks took 7.546,
7.349, and 7.589 seconds, giving a median of 7.546 seconds. Looking up the newly
prepared leaf and checking its directory identity took 61.945, 8.263, and 81.639
milliseconds, giving a median of 61.945 milliseconds. Prepared lookup excludes
creation and has no atomic worker-assignment semantics. The independent oracle
completed 194,656 file comparisons, checked directory names/modes/mtimes, and
preserved the source and all six preexisting siblings.

Reusing the pinned source manifest while retaining a final hash of the target
source changed an earlier matched three-trial median from 20.587 to 8.990 seconds.
The original run ranged from 16.049 to 32.769 seconds; the changed run ranged from
8.759 to 10.115 seconds. A later repeat with six through eight existing leaves
measured 10.948 seconds, ranging from 9.578 to 11.211 seconds. The difference
between repeated series is part of the observed variability, not evidence of a
universal nine-second latency. The earlier series performed 106,176 independent
file comparisons. The final result also uses a cached native function binding,
but its fixed leaf count and shared-host cache state differ from that later
repeat; the 10.948-to-7.546-second difference cannot be attributed to binding
caching alone.

A diagnostic profile of the later fork path took 12.135 seconds. Three tree scans
accounted for 4.512 seconds cumulatively, native clone calls for 2.374 seconds,
workspace sessions for 2.075 seconds, origin pinning for 1.353 seconds, and nine
Git commands for 0.921 seconds. File reads accounted for 0.971 seconds. These
categories are nested and overlap; summing them would double count work. Profiling
adds overhead and is used to locate costs, not as an uninstrumented latency result.

Prepared-leaf handoff is a separate operation. An already prepared path can be
assigned without repeating import or fork, but the caller must exclusively assign
it to one worker and account for replenishment cost. Cowtree's path-publication
lease is not a worker-assignment lock.

The bundled CLI, using Python 3.14.7 and the release metadata service, inherited
108 entries in a separate two-crate smoke fixture. In this single functional run,
import took 0.765 seconds and fork took 0.393 seconds; the initial and unchanged
Cargo builds took 0.117 and 0.122 seconds, each reporting three fresh artifacts and
zero compilations. Editing the dependency rebuilt two artifacts and changed the
program output from ``42:seed`` to ``43:seed`` in 0.232 seconds while preserving
the original source and cache. This small-fixture qualification does not compare
bundle performance with the source CLI or establish large-project fork latency.

Native clone experiment
-----------------------

An isolated macOS probe cloned 1,200 sampled Ruff files totaling 37,454,829 bytes
with 574 parent directories. All bindings were cached before the loop. Three
rotated trials gave median materialization times of 0.322 seconds serially,
0.297 with four threads, 0.255 with 16 threads, and 0.339 with 64 threads. That
small benefit does not justify extrapolating a large thread-pool speedup.

The same experiment evaluated recursive ``clonefile`` on an absent destination.
Raw recursive cloning changed every sampled directory's modification time, so
the oracle refused that result. Explicit directory mode/time restoration reduced
the comparison to a valid metadata-preserving materialization. Apple documents
recursive ``clonefile`` but discourages its use for directory trees.
`Apple clonefile contract <https://github.com/apple-oss-distributions/xnu/blob/main/bsd/man/man2/clonefile.2>`_

The checked-in directory probe ran three trials on the complete frozen Ruff node.
Direct recursive cloning took 0.274, 0.198, and 0.188 seconds. Including cached
directory metadata restoration gave 0.350, 0.299, and 0.256 seconds, a median of
0.299 seconds. All 115,932 source/target entry comparisons passed, including
directory metadata and the root directory. The separate full target oracle took
4.210 to 4.375 seconds; it hashes derived cache payloads as well as source, which
exceeds the production source-only hash check.

This establishes subsecond native materialization on this prepared tree. It does
not establish subsecond acknowledged managed forks or an absolute lower bound
for every implementation. Git registration, source-manifest validation, authority
updates, and durability acknowledgement are absent from the native timer.
Direct integration would require a new staging/registration sequence because
the managed target already contains a Git control entry, plus crash, permission,
source-mutation, and collision qualification. It must also preserve explicit
policy exclusions and independent per-path cache inodes after hard-link cloning.
Apple's directory-clone warning supplies no rationale; the successful probe does
not establish those missing workspace contracts.

``copyfile`` is not an equivalent strict replacement: recursive
``COPYFILE_CLONE_FORCE`` returned ``EINVAL``, while recursive ``COPYFILE_CLONE``
permits ordinary copying when cloning fails. Cowtree requires an explicit native
clone and propagates unsupported-operation errors.
`Apple copyfile contract <https://github.com/apple-oss-distributions/copyfile/blob/main/copyfile.3>`_

Reproduction
------------

Build and select one metadata executable before measuring. Put generated evidence
outside the repository and use absent benchmark roots on the same filesystem as
the source::

    cargo build --locked --release -p cowtree-metadata
    PYTHONPATH=src uv run --group test python benchmarks/import_workspace.py \
        --root /cow-volume/import-evidence --binary target/release/cowtree-metadata \
        --files 128 --trials 3

To measure a new managed fork and a subsequent prepared-leaf lookup separately,
use an existing quiescent store and its initial or explicitly retained node::

    PYTHONPATH=src uv run --group test python -m benchmarks.fork_workspace \
        --store /cow-volume/store --node RETAINED_NODE_ID \
        --root /cow-volume/fork-evidence --trials 3

This harness checks the frozen node, source, and existing siblings, and retires
only its own completed trial leaves. Prepared lookup time excludes creation and
does not provide atomic assignment to a worker.

For real Cargo builds, use the fixture and private-cache procedure in
`Cargo workspaces <cargo-workspaces.rst>`_. Report import, priming, fork, initial
build, and edited build separately. An unchanged build and a dependency-changing
build answer different questions.

To repeat the experimental native phase on a prepared tree without Git control
files, run on macOS::

    PYTHONPATH=src uv run --group test python -m benchmarks.clone_directory \
        --source /cow-volume/frozen-node/tree --root /cow-volume/clone-evidence \
        --trials 3

The checked-in probe streams its independent hashes, records file and directory
counts, verifies exact names and metadata, and removes only successfully verified
trial clones. Failed clones remain available for inspection. It does not publish
a workspace or introduce a recursive-clone production backend.

Interpretation
--------------

Correctness tests should assert invariants rather than machine-specific timing
thresholds. Run the mounted corruption, fork, import interruption, and collection
tests on the actual deployment filesystem. APFS timing cannot predict Btrfs, XFS,
or ReFS timing. Process-kill recovery does not prove machine or storage power-loss
recovery. Flush-request measurements and their ordering are described in
`Group durability <group-durability.rst>`_ and
`Durability benchmark <durability-benchmark.rst>`_.

BSMR must continue deciding action identities and cache validity. These results
qualify private filesystem views and specific compiler reuse cases; they do not
make every inherited output valid for a different toolchain, environment, target,
or absolute build path.
