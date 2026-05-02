cowtree
=======

::

            (__)
            (oo)
     /-------\/
    / |     ||
   *  ||----||
      ~~    ~~

copy-on-write git worktrees, because agent fleets should not turn your
checkout into sedimentary disk geology.

Few notes on how to use cowtree in short steps:

::

    $ cd /path/to/git/repo
    $ cowtree add -b scratch/idea ../wt/idea HEAD
    $ cd ../wt/idea

Or, if ``git-cowtree`` is on ``PATH``:

::

    $ git cowtree add -b scratch/idea ../wt/idea HEAD

That's all. Enjoy.

What it is
----------

``cowtree`` creates a normal Git worktree, but populates tracked files with
filesystem copy-on-write clones instead of asking Git to write every file from
the object database.

The public interface is intentionally small:

* ``cowtree add <git-worktree-add-options> <path> [commit-ish]``
* ``cowtree list [--json]``
* ``cowtree remove [--force] <path>``
* ``cowtree doctor [path]``

The Python API uses typed frozen dataclasses and subprocess argv lists. No shell
strings are part of the contract.

What it is not
--------------

``cowtree`` is not a branch dashboard, TUI, process manager, or agent
orchestrator. Wingman and other tools should build those workflows on top.

It is also not portable magic. CoW support belongs to the filesystem:

* macOS: APFS via ``clonefile(2)``
* Linux: filesystems where ``cp --reflink=always`` works
* ext4: no

If CoW is unavailable, cowtree fails.

Development
-----------

::

    $ uv sync --group dev
    $ uv run ruff check .
    $ uv run ruff format --check .
    $ uv run pytest
    $ uv run ty check

Install local hooks with:

::

    $ uv run prek install

Inline tests live beside the code and are stripped from builds by
``inline-tests``.

See ``benchmarks/`` for the space-efficiency plots and
``docs/filesystems.rst`` for the filesystem validation matrix. See
``docs/rust-extension.rst`` for the optional Rust acceleration path.
