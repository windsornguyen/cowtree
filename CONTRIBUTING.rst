contributing
============

Use small commits with conventional commit messages.

Use ``uv`` for package management. Do not use ``pip``, ``poetry``, or
``requirements.txt`` in this repo.

There is no CLA.

PRs from unknown contributors are gated by Vouch. Ask for a maintainer to vouch
for you on an issue before opening a substantial PR.

Setup
-----

::

    $ uv sync --group dev
    $ uv run prek install

Checks
------

::

    $ uv lock --check
    $ uv run ruff check .
    $ uv run ruff format --check .
    $ uv run ty check
    $ uv run pytest
    $ uv build

Docs changes should also build the docs site:

::

    $ uv run --group docs mkdocs build --strict

Dependencies
------------

Use ``uv add`` and ``uv remove`` for dependency edits. Use ``--group`` for
local development tools and ``--optional`` for published extras.

Do not edit ``uv.lock`` by hand.

Benchmark changes should also regenerate plots:

::

    $ uv run python benchmarks/run.py --preset quick --runs 2
    $ uv run python benchmarks/plot.py benchmarks/results/latest.json

Scope
-----

``cowtree`` should stay narrow. It creates and manages CoW-backed Git
worktrees. Agent orchestration, dashboards, branch policy, and process
management belong in callers such as Wingman.
