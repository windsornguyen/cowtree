cowtree
=======

``cowtree`` creates normal Git worktrees with native copy-on-write (CoW) clones
of tracked files. Cloned regular files initially share storage; writing to one
does not modify the other.

Managed workspaces
------------------

``cowtree workspace`` adds warm cache inheritance, retained private checkpoints,
checked source publication, synchronization, recovery, and collection. The Rust
SQLite service owns publication; Python owns the real filesystem installation.

Start with the `managed workspace guide <docs/managed-workspaces.rst>`_. It lists
the complete CLI and Python API, JSON failures, editor coordination rules, and
a runnable workflow. The original tracked-file commands below remain supported.

`Build-cache measurements <docs/warm-cache.rst>`_ demonstrate actual compiler
reuse and report total creation cost. `Lifecycle qualification
<docs/workspace-qualification.rst>`_ checks independently predicted file states
through publication and teardown. `Verification <docs/verification.rst>`_
describes model checks, implementation replay, and their limits.

Command line
------------

From a clean Git checkout::

    cowtree add -b scratch/idea ../wt/idea HEAD
    cowtree list --json
    cowtree remove ../wt/idea

The supported commands are::

    cowtree add [-b NEW_BRANCH | --detach] [--lock] [--reason TEXT]
                [--] path [commit-ish]
    cowtree list [--json] [--] [source]
    cowtree remove [--force] [--] path
    cowtree doctor [--] [directory]
    cowtree help

``git cowtree`` exposes the same commands when ``git-cowtree`` is on ``PATH``.
All commands accept ``--help`` and ``-h``. ``--`` ends option parsing, including
for paths that start with ``-``.

``add`` uses the checkout containing the current directory as its source.
It creates a detached worktree by default. ``--detach`` and its alias ``-d``
request that behavior explicitly; ``-b`` creates a new branch instead.
``commit-ish`` defaults to ``HEAD`` and must resolve to the source's current
commit. ``--lock`` retains Git's worktree lock. ``--reason`` requires ``--lock``;
an omitted reason remains ``None`` in the returned metadata.

Use ``add --branch EXISTING_BRANCH PATH`` to attach an unused local branch
instead of creating one. It is mutually exclusive with ``-b`` and ``--detach``.
The branch must name the source commit; Git rejects a branch already checked out
elsewhere. The Python equivalent is ``WorktreeAddRequest(existing_branch=...)``.
Neither failed creation nor removal deletes an existing branch. Index creation
never resets its reference, including when an external writer moves it.

Use ``add --committed PATH REF`` to fork a committed ref even when the caller's
checkout is dirty or at a different commit. With ``--branch``, that branch's tip
selects the commit; omit REF. The Python request uses
``source_mode=SourceMode.COMMIT`` from ``cowtree.types``. The default remains
``SourceMode.CHECKOUT`` and preserves the clean-source requirement.

Committed mode resolves the ref once, materializes a private Git checkout, and
clones its files through native CoW. It never stashes or resets the caller's
working files or index. Git applies checkout conversions in the private seed.
The seed is removed before success, so no extra checked-out payload is retained.
This mode pays Git checkout cost and does not provide warm-cache inheritance.
Unsupported clone storage still fails rather than copying into the destination.
If seed cleanup fails, inspect the named directory and ``git worktree list``:
the destination may already be complete. Unhandled process crashes can leave the
locked private seed behind, under the same standalone recovery limits below.

Cowtree always populates tracked files through CoW and prepares a clean index.
There is no pass-through for arbitrary Git arguments. ``-B``, ``--force`` on
``add``, ``--orphan``, ``--cow``, ``--no-checkout``, tracking options, and
unknown options are rejected.

``list`` accepts a source checkout or defaults to the current repository.
Use ``--json`` for machine-readable output, including paths containing line
breaks. ``add``, ``doctor``, and ``remove`` also accept ``--json``: successes
contain ``status: "ok"``, ``kind``, and ``value``. Add returns the registered
worktree, doctor returns its probe report, and remove returns null. An unsupported
doctor probe has a null clone tool and still exits 1. Structured failures emit
one ``status: "error"``, ``code``, and ``message`` record on stderr; invalid
syntax exits 2. ``list --json`` retains its existing array format.

``remove`` operates in the current repository and preserves branches.
``--force`` permits removal of a dirty worktree, but does not override a worktree
lock. Unlock it with ``git worktree unlock PATH`` first. ``doctor`` probes an
existing directory and defaults to the current directory.

Exit codes are ``0`` for success, ``1`` for an operation failure or unsupported
doctor result, and ``2`` for invalid command syntax.

Python API
----------

Import the four public operations from ``cowtree.core`` and their request and
result types from ``cowtree.types``. Their signatures are::

    add_worktree(request: WorktreeAddRequest,
                 runner: CommandRunner | None = None) -> Worktree
    list_all_worktrees(source: Path | None = None,
                      runner: CommandRunner | None = None) -> list[Worktree]
    remove_worktree(path: Path, *, source: Path | None = None,
                    force: bool = False,
                    runner: CommandRunner | None = None) -> None
    inspect_path(path: Path,
                 runner: CommandRunner | None = None) -> DoctorReport

The optional ``CommandRunner`` from ``cowtree.exec`` supports command execution
in tests. Callers normally omit it.

``WorktreeAddRequest`` is a frozen dataclass with this constructor::

    WorktreeAddRequest(
        path: Path,
        source: Path | None = None,
        branch: str | None = None,
        commitish: str = "HEAD",
        detach: bool = False,
        lock: bool = False,
        reason: str | None = None,
    )

``path`` is required. Relative paths resolve from the caller's current
directory, including when ``source`` names another checkout. A missing
``source`` selects the current repository. ``branch=None`` means detached;
``detach=True`` is an explicit equivalent. ``branch`` and ``detach=True``
conflict. ``reason`` requires ``lock=True``. String fields must be nonempty
when supplied and cannot contain NUL bytes.

This replaces the earlier ``WorktreeAddRequest(args=[...])`` API. Migrate each
supported option to its named field; raw Git arguments are no longer accepted.

For example:

.. code-block:: python

    from pathlib import Path

    from cowtree.core import add_worktree, remove_worktree
    from cowtree.types import Worktree, WorktreeAddRequest

    source: Path = Path("/path/to/repo")
    request: WorktreeAddRequest = WorktreeAddRequest(
        path=Path("/path/to/worktrees/idea"),
        source=source,
        branch="scratch/idea",
    )
    worktree: Worktree = add_worktree(request=request)
    remove_worktree(path=worktree.path, source=source)

Return values
~~~~~~~~~~~~~

``add_worktree`` returns the registered ``Worktree`` after cloning and index
validation. ``list_all_worktrees`` returns every worktree reported by Git,
including the main checkout and worktrees created outside cowtree.

``Worktree`` is frozen and contains ``path: Path``, ``head: str | None``,
``branch: str | None``, ``detached: bool``, ``prunable: bool``, ``locked: bool``,
and ``reason: str | None``. Branch names are full refs such as
``refs/heads/scratch/idea``. ``reason`` is the optional lock reason.
``to_json_text()`` serializes these fields with ``path`` as a string;
``cowtree list --json`` returns an array of the same objects.

Paths use Git's registered spelling. On filesystems that treat different Unicode
spellings as the same name, cowtree matches existing directories by filesystem
identity during creation and rollback. Missing unrelated registrations do not
prevent an independent add.

``inspect_path`` returns a frozen ``DoctorReport`` with ``path: Path``,
``filesystem: FilesystemKind``, ``clone_tool: CloneTool | None``, and
``reason: str | None``. The enums live in ``cowtree.types``. Filesystem values
are ``apfs``, ``reflink``, or ``unsupported``. ``supported`` is true only when
a native clone probe succeeds and ``clone_tool`` is present. An unsupported
filesystem returns a report with ``supported=False``; invalid directories and
operational probe failures raise an error.

Requirements
------------

The source must be a normal, complete checkout with a committed ``HEAD`` and
no tracked changes. A linked worktree can serve as the source. Sparse checkouts,
submodules, and index entries marked assume-unchanged or skip-worktree are
rejected. Untracked and ignored files do not affect eligibility and are not
copied. Tracked regular files, executable modes, and symlinks are supported.
Symlinks retain their original link text and normal Git checkout semantics.

The requested commit must equal the source's current commit. A requested branch
must not already exist. The destination must not exist, even as an empty
directory or dangling symlink. Missing parent directories are created. Source
and destination must be on the same filesystem.

Native clone mechanisms are:

* macOS APFS: ``clonefile(2)``.
* Linux filesystems supporting reflinks: ``FICLONE`` directly, without ``cp``.

A real clone probe establishes support. Filesystem names alone are insufficient.
Cowtree has no ordinary-copy fallback. Run ``cowtree doctor DIRECTORY`` on the
intended mount; see `filesystem validation <docs/filesystems.rst>`_ for the
qualification matrix.

Failures and concurrency
------------------------

Operations raise ``CowtreeError`` with a ``CowtreeErrorCode`` in ``code`` and
a diagnostic string in ``message``. Import both types from ``cowtree.errors``.

.. list-table:: Error codes
   :header-rows: 1
   :widths: 28 72

   * - Code
     - Meaning
   * - ``invalid_arguments``
     - Invalid request fields, paths, conflicts, or an existing destination or branch.
   * - ``dirty_source``
     - Tracked changes, hidden index flags, or a source change detected during cloning.
   * - ``head_mismatch``
     - The requested commit differs from source HEAD, or HEAD changes during cloning.
   * - ``sparse_checkout``
     - The source uses sparse checkout.
   * - ``submodule_unsupported``
     - The source commit contains a submodule.
   * - ``unsupported_mode``
     - The source commit contains an unsupported tracked file mode.
   * - ``different_filesystem``
     - Source and destination are on different filesystems.
   * - ``cow_unavailable``
     - The native filesystem clone operation is unavailable.
   * - ``command_failed``
     - A Git, process, or filesystem operation fails.
   * - ``worktree_not_found``
     - Git returns a worktree record without a path.
   * - ``cleanup_failed``
     - An add fails and its rollback also fails; retained state requires inspection.

When an add fails after creating its destination, cowtree synchronously removes
its registration and destination, and deletes the new branch requested by that
add. A failed rollback raises ``cleanup_failed`` with the original failure and
cleanup context. Inspect ``git worktree list`` and the named path and branch
before retrying. Rollback also removes empty parent directories created by that
add; it preserves pre-existing directories.

Keep the source quiescent, meaning no changes to tracked files, the index, or
HEAD during an add. Cowtree checks for changes, but does not take an atomic
snapshot across all source files.

Cowtree serializes each complete ``add``, ``list``, and ``remove`` operation with
a repository lock shared by its worktrees. This includes the full clone, so
concurrent adds to one repository wait for each other. The lock coordinates
cowtree callers only. Raw Git commands and file writes do not acquire it.

Rollback covers exceptions handled by the running process. There is no crash
recovery guarantee for ``SIGKILL``, power loss, or a host crash. Such failures
can leave a partially initialized, locked worktree or a newly created branch.
Inspect and repair that state with Git before reusing the destination.

Workspace protocol design
-------------------------

The `versioned workspace design <docs/workspace-protocol.rst>`_ explains the
immutable snapshots, path reservations, fencing tokens, and batched publication
used by managed workspaces. The `managed workspace guide
<docs/managed-workspaces.rst>`_ defines the available CLI and Python operations.
Transparent file-descriptor rebinding, native change tracking, and distributed
coordination remain future work. Bounded model checks do not establish those
capabilities or power-loss durability.

Contributing
------------

New external contributors must open a
`Contribution interest issue <https://github.com/windsornguyen/cowtree/issues/new?template=contribution.yml>`_
and receive a maintainer's vouch before opening a PR. See the
`contribution guide <CONTRIBUTING.rst>`_ for the process, existing-contributor
access, and development checks. Bug reports and questions do not require a vouch.

Development
-----------

Use ``uv`` for dependencies, tests, linting, and builds::

    uv sync --group dev
    uv run ruff check .
    uv run ruff format --check .
    uv run ty check
    uv lock --check
    uv run pytest
    uv build

Require native CoW support instead of allowing unsupported-filesystem skips::

    COWTREE_EXPECT_SUPPORTED=1 uv run --group test pytest -q

Run the Linux filesystem matrix in a disposable VM using the prerequisites and
commands in `filesystem validation`_. That document also describes mount
selection, concurrency stress controls, and the limits of the tests.

Integration tests live in ``tests/``. Inline tests live beside the code and are
stripped from builds by ``inline-tests``. Install local hooks with
``uv run prek install``. See `benchmarks <benchmarks/>`_ for benchmark fixtures
and the `Rust extension proposal <docs/rust-extension.rst>`_ for optional
acceleration.

Batched publication verification
---------------------------------

The `FencedPublish checks <specs/FencedPublish.rst>`_ complement the workspace
model with captured proposal tokens, disjoint batches, and finite induction.
They validate a bounded protocol model and do not add worktree API operations.

Local metadata authority
------------------------

The optional Rust ``cowtree-metadata`` crate implements a single-host SQLite WAL
metadata authority with fenced path reservations, immutable snapshots, retry
receipts, and retention. Its library and JSON operation contracts are documented
in `crates/cowtree-metadata/README.md <crates/cowtree-metadata/README.md>`_.
Run the complete publication example with::

    cargo run --locked -p cowtree-metadata --example publish

The standalone ``cowtree.core`` API does not use this backend. The managed
``cowtree.workspace.Workspace`` API uses it for publication and coordinates
filesystem installation in Python. Calling the Rust authority directly changes
only its logical view; it does not install files in a working directory.
