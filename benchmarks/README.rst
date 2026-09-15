cowtree benchmarks
==================

These benchmarks compare normal ``git worktree add`` against ``cowtree add``.
They measure real commands on a temporary Git repository.

The north star is space efficiency: how much redundant checked-out payload does
the workflow avoid when many branches are active at once? Wall time is reported
because it matters, but it is secondary for this project.

Run
---

::

    $ uv sync --group dev
    $ uv run python benchmarks/run.py --preset quick --runs 2
    $ uv run python benchmarks/plot.py benchmarks/results/latest.json

For a heavier run:

::

    $ uv run python benchmarks/run.py --preset full --runs 3

Profiles
--------

``basic``
    One worktree. Small repo. This is the everyday "try one thing" case.

``average``
    Several worktrees. Medium repo. This is the normal AI-agent branching case.

``intensive``
    Many worktrees. Larger repo. This is the "agent fleet" case where ordinary
    full checkout writes become annoying.

Plots
-----

``plots/elapsed_seconds.png``
    Median total create time for all worktrees in the profile.

``plots/time_ratio.png``
    Median ``git worktree add`` time divided by median ``cowtree add`` time.
    Values above ``1.0`` mean ``cowtree`` was faster; values below ``1.0`` mean
    Git was faster.

``plots/payload_copied.png``
    Logical tracked payload that the checkout path materializes. ``cowtree``
    still writes metadata; it does not copy file contents when CoW is available.

``plots/payload_avoided.png``
    Redundant checked-out payload avoided by using CoW clones.

Notes
-----

APFS and reflink filesystems do not expose a simple per-directory "new physical
bytes allocated by this clone" number. ``du`` and ``st_blocks`` count shared
extents per file, which overstates CoW clones. ``run.py`` therefore reports
logical tracked payload copied by the checkout path. The APFS harness below
measures physical container allocation. Treat the wall-time charts
as supporting evidence, not the claim.

Physical APFS allocation
------------------------

Recorded Linux v6.12 results: `APFS space report
<results/apfs-linux-2026-09-15/report.md>`_.

``space.py`` measures physical allocation in an isolated case-sensitive APFS
sparse image on macOS. It counts shared extents once, including filesystem
metadata. It uses the same source and random operation plan for Git and cowtree.
Each arm has its own image. No existing worktree is modified.

Prepare a pinned public workload::

    $ git clone --bare --depth 1 --branch v6.12 https://github.com/torvalds/linux.git /tmp/linux.git
    $ uv sync --group dev
    $ uv run python -m benchmarks.space --source /tmp/linux.git \
        --commit adc218676eef25575469234709c2d87185ca223a \
        --output /tmp/cowtree-space-pilot --leaves 1 --trials 1 --capacity-gib 16

Run three paired four-worktree trials after the pilot succeeds::

    $ uv run python -m benchmarks.space --source /tmp/linux.git \
        --commit adc218676eef25575469234709c2d87185ca223a \
        --output /tmp/cowtree-space --leaves 4 --trials 3
    $ uv run python -m benchmarks.space_report /tmp/cowtree-space \
        --output /tmp/cowtree-space-report

Use a new output directory. Every arm retains its detached image and raw JSON
measurements, operation history, and source manifest. Images grow on demand;
each is capped at 32 GiB by default. Before creating an image, the harness
requires its full capacity plus an 80 GiB host reserve. Completed images are
retained for inspection, so additional trials require additional host space.
Delete only a completed run's detached ``*.sparseimage`` files to reclaim them.

Measurements use APFS container capacity counters after a normal detach and
reattach, followed by three identical samples. This flushes deferred allocation.
Per-file ``du`` is not used for savings. Sparse-image host allocation is also
recorded: it can retain freed space and is a separate high-water footprint.
Empty image and source-only measurements distinguish additional-fleet savings
from total source-plus-fleet savings. Ratios are measured, never assumed or
clamped. Operation times exclude verification and image checkpoints.

Each active tree receives atomic editor saves to a seeded 1% sample of tracked
regular C/header files. The benchmark then performs two rounds of random
remove/recreate and in-place comment appends. One directory is deleted externally
before explicit API registry cleanup. Full verification hashes every tracked
file, file mode and symlink before edits and after the final round, checks exact
path inventories, and verifies the unchanged source. Intermediate verification
hashes all edited files plus about 128 clean paths, and checks HEAD, dirty paths,
and the exact worktree registry. The paired arms must produce identical states.

These are sequential lifecycle and isolation checks. They do not claim process
crash recovery, concurrent linearizability, or successful compilation of the
modified project. The workload appends C comments and does not execute downloaded
project code. Results depend on file-size distribution, metadata overhead, edit
fraction, and how the editor writes files. A universal 99% physical saving does
not follow from avoiding 99% of file-content writes.
