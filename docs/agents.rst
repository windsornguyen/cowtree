Git commands for agents
=======================

An agent needs a private checkout before it edits files. Cowtree creates a
normal Git worktree while sharing unchanged file storage. The agent can use
its existing editor, compiler, and Git commands inside that directory.

Install once
------------

Install Cowtree and verify the Git extension in the same environment that
launches the agent::

    uv tool install git+https://github.com/windsornguyen/cowtree
    git cowtree --version
    git cowtree doctor .

The installer supplies both ``cowtree`` and ``git-cowtree``. Git discovers
``git-cowtree`` on ``PATH`` when invoked as ``git cowtree``. If Git cannot find
it, add the directory printed by ``uv tool dir --bin`` to the agent's PATH.

Change the worktree commands
----------------------------

Configure the agent's create, inspect, and remove commands as follows::

    git cowtree add --committed -b agent/fix ../wt/fix HEAD
    git cowtree list --json
    git cowtree remove ../wt/fix

Use a distinct branch and destination for each task. ``--committed`` selects
the named commit and leaves the source checkout's staged and unstaged edits
alone. It materializes a temporary seed, so it pays Git checkout cost. The
default clean-checkout mode can clone directly and avoids that seed.

Git's ``-C`` selects the repository before the extension runs::

    git -C "/path/to/repo" cowtree list --json

For a shorter command, choose an unused alias and configure it in that repo::

    git config --local alias.wt cowtree
    git wt add --committed -b agent/fix ../wt/fix HEAD
    git wt list --json
    git wt remove ../wt/fix

This alias expands through Git directly. It does not depend on an interactive
shell function. Undo it with ``git config --local --unset alias.wt``.

Instructions for an agent
-------------------------

Add this block to the project's agent instructions or worktree tool config::

    Create task worktrees with:
      git cowtree add --committed -b <branch> <path> <ref>
    Inspect them with git cowtree list --json.
    Remove only this task's finished worktree with git cowtree remove <path>.
    Keep ordinary Git commands for commits, diffs, branches, and pushes.
    On a Cowtree error, report the failure. Do not retry with git worktree add.
    Preserve dirty worktrees unless their removal is explicitly authorized.

Existing Git worktrees remain visible. Removing a worktree preserves its
branch. Filesystem sharing isolates writes through each worktree's own files.
It does not restrict a process from opening another agent's directory.

Compatibility boundary
----------------------

This is a command-template change, not a complete replacement for every
``git worktree`` option. In particular:

* Cowtree creates a detached worktree by default. Pass ``-b`` for a new branch
  or ``--branch`` to attach an unused local branch. Git's implicit branch
  selection is different.
* Cowtree's machine-readable listing is JSON. Keep ``git worktree list
  --porcelain -z`` if a caller requires Git's record format.
* Use ``--force`` only on removal when discarding that worktree's edits is
  intended. Creation rejects ``--force``, ``-B``, and unknown options.
* Tracked files are cloned. Use `managed workspaces <managed-workspaces.rst>`_
  to inherit ignored build caches and retain checkpoints.
* Unsupported filesystems and cross-filesystem destinations fail. A caller
  that falls back to ordinary checkout changes the storage contract.

``git config alias.worktree cowtree`` cannot override ``git worktree``.
Git ignores aliases that hide built-in commands. An agent that hardcodes
``git worktree`` must change its command template to use this integration.
See `Git's alias rules <https://git-scm.com/docs/git-config#Documentation/git-config.txt-alias>`_
and `worktree branch defaults <https://git-scm.com/docs/git-worktree#_commands>`_.

Verification
------------

``tests/test_git_extension.py`` invokes the installed Git extension and its
repo-local alias in disposable repositories. It checks relative paths from
``git -C``, paths containing spaces, source and index preservation, dirty-removal
refusal, branch retention, and argument rejection before state changes.
On macOS or Linux with native cloning, run::

    uv sync --group dev
    COWTREE_EXPECT_SUPPORTED=1 uv run pytest -q tests/test_git_extension.py

The Windows ReFS jobs run the same tests on x64 and ARM64 through
``scripts/test_windows.ps1``.
