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
extents per file, which overstates CoW clones. This benchmark therefore reports
logical tracked payload copied by the checkout path. Treat the wall-time charts
as supporting evidence, not the claim.
