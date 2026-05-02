contributing
============

Use small commits with conventional commit messages.

Setup
-----

::

    $ uv sync --group dev
    $ uv run prek install

Checks
------

::

    $ uv run ruff check .
    $ uv run ruff format --check .
    $ uv run ty check
    $ uv run pytest
    $ uv build

Benchmark changes should also regenerate plots:

::

    $ uv run python benchmarks/run.py --preset quick --runs 2
    $ uv run python benchmarks/plot.py benchmarks/results/latest.json

Scope
-----

``cowtree`` should stay narrow. It creates and manages CoW-backed Git
worktrees. Agent orchestration, dashboards, branch policy, and process
management belong in callers such as Wingman.
