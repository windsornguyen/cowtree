Physical space efficiency
=========================

.. note::

   Historical measurements on this page describe their recorded revisions.
   Their original harnesses remain in Git history. For the Rust-only runtime,
   use ``cargo test --workspace --all-features`` and the native benchmark in
   ``benchmarks/README.rst``. Earlier qualification is not a result for a later refactor.

A matched Linux v6.12 workload on APFS measured 96 to 97 percent savings in
additional-worktree allocation. It did not support a universal 99 percent
physical-space claim. These are observations from a specific earlier Cowtree
revision, not a qualification of every later release.

Recorded figure
---------------

The four-worktree figure below is the original recorded observation. Its URL
is pinned to the archive commit, not to the current runtime. The two panels
separate additional-worktree allocation from total source-plus-fleet allocation.

.. image:: https://raw.githubusercontent.com/windsornguyen/cowtree/cf8fef1b390e8a12ab3e81ee3aeabeee07e0184e/benchmarks/results/apfs-linux-2026-09-15/fleet/space-savings.png
   :alt: Git and cowtree allocated GiB for four extra worktrees, before and after edits.

`Recorded measurements and report <https://github.com/windsornguyen/cowtree/tree/cf8fef1b390e8a12ab3e81ee3aeabeee07e0184e/benchmarks/results/apfs-linux-2026-09-15/fleet>`_
include the source revision, paired operation results, and allocation samples.
Generated results remain outside the current source tree.

Method
------

The input was public Linux commit ``adc218676eef25575469234709c2d87185ca223a``:
86,680 tracked files, 1,476,500,559 tracked bytes, and 62 symlinks. Each leaf received
atomic saves to 600 of the 59,953 regular C/header files, selected with seed 1729.
Both Git and Cowtree used the same edit and teardown sequence.

The measured Cowtree revision was ``f97d7ccc8ff784689ac077af5e782296d0c752d2``.
The host used an Apple M5 Max, 48 GiB memory, macOS 26.5.2 arm64, Python 3.10.20,
and Git 2.50.1. Each method used a private case-sensitive APFS sparse image.
Case sensitivity preserved the input's 13 groups of colliding path spellings.

Normal detach and reattach preceded three equal container-used samples. Empty
image and source-only baselines separated additional-tree allocation from total
source-plus-fleet allocation. Filesystem metadata and Git indexes counted toward
the result. Per-file ``du`` was not used: it counts shared extents repeatedly.
Sparse-image allocation on the host was a separate high-water measurement.

The meter calibration used a dense 128 MiB file. Cloning added 16 KiB, changing
1 MiB added approximately 1 MiB, and ordinary copying added approximately 128 MiB.
Unchanged mount checkpoints drifted by 8 KiB.

Observed results
----------------

Each fleet size has one paired observation, with Git first. There are no repeat
statistics. Later stages include metadata written by earlier Git status checks.

.. list-table:: Allocated additional-worktree space
   :header-rows: 1

   * - Leaves
     - State
     - Git GiB
     - Cowtree GiB
     - Additional-tree saving
     - Source plus fleet saving
   * - 1
     - Pristine
     - 1.616
     - 0.041
     - 97.48%
     - 45.18%
   * - 1
     - After atomic saves
     - 1.617
     - 0.063
     - 96.09%
     - 44.54%
   * - 1
     - After two churn rounds
     - 1.620
     - 0.053
     - 96.70%
     - 44.87%
   * - 4
     - Pristine
     - 6.464
     - 0.163
     - 97.47%
     - 75.60%
   * - 4
     - After atomic saves
     - 6.466
     - 0.238
     - 96.32%
     - 74.71%
   * - 4
     - After two churn rounds
     - 6.473
     - 0.223
     - 96.55%
     - 74.90%

Additional-tree saving is one minus the ratio of Cowtree to Git allocation after
subtracting each method's source-only baseline. Total saving subtracts each empty
filesystem baseline instead. The two percentages answer different questions.

Correctness and limitations
---------------------------

Full byte, mode, symlink, and path inventories were checked before editing and
after final churn. Intermediate checks covered every edited file and roughly
128 clean paths per leaf, Git HEAD, dirty paths, and exact worktree registration.
Both methods had to produce equal operation results and state digests. Two churn
rounds removed and recreated selected leaves. One directory was deleted externally
before API cleanup. The source was checked again after all leaves were removed.

The one-leaf run completed 1,816 operations per method. The four-leaf run completed
7,226. Its final inventory hashed 433,400 file entries and checked 4,182 changed
leaf files. Registrations and leaf directories were absent after cleanup, and
source hashes remained unchanged.

This was a sequential isolation and lifecycle experiment. It does not establish
concurrent linearizability, process-crash or power-loss recovery, or successful
compilation of the modified Linux sources. Sparse-image I/O and unrelated host
load prevent using its timings as native-volume speed claims. Allocation depends
on file sizes, editor write behavior, and the amount of divergence.

Reproduction
------------

To regenerate the figure from the recorded data, run from a checkout containing
the archive commit below. This redraws an old observation. It does not rerun the
benchmark or measure current code::

    evidence=$(mktemp -d)
    git archive cf8fef1b390e8a12ab3e81ee3aeabeee07e0184e \
        benchmarks/results/apfs-linux-2026-09-15/fleet | tar -x -C "$evidence"
    uv run --group bench python -m benchmarks.space_report \
        "$evidence/benchmarks/results/apfs-linux-2026-09-15/fleet" \
        --output "$evidence/plots"

Open ``$evidence/plots/space-savings.png`` or ``$evidence/plots/report.md``.

Follow the `APFS benchmark procedure <../benchmarks/README.rst#physical-apfs-allocation>`_
to obtain the pinned source, run identical seeded histories, and generate a report
outside the checkout. Use three paired trials with alternating method order for
a new comparison. The observations above used ``--trials 1`` explicitly.
Retain raw measurements and checksums as separate artifacts. Commit changes to
the methodology and reviewed conclusions. Repeat qualification for the release
and deployment being considered.
