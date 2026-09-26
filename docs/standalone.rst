Standalone worktrees
====================

``cowtree`` creates normal Git worktrees with native copy-on-write (CoW) clones
of tracked files. Cloned regular files initially share storage. Writing to one
does not modify the other.

This page covers the tracked-file API. For warm build caches, checkpoints,
checked publication, and recovery, see `managed workspaces <managed-workspaces.rst>`_.

Command line
------------

From a clean Git checkout::

    cowtree add -b scratch/idea ../wt/idea HEAD
    cowtree list --json
    cowtree remove ../wt/idea

The supported commands are::

    cowtree add [-b NEW_BRANCH | --branch EXISTING_BRANCH | --detach]
                [--committed] [--lock] [--reason TEXT] [--json]
                [--submodules reject|leave-uninitialized|materialize-pinned]
                [--] path [commit-ish]
    cowtree list [--json] [--] [source]
    cowtree remove [--force] [--json] [--] path
    cowtree doctor [--json] [--] [directory]
    cowtree --version [--json]
    cowtree help

``git cowtree`` exposes the same commands when ``git-cowtree`` is on ``PATH``.
All commands accept ``--help`` and ``-h``. ``--`` ends option parsing, including
for paths that start with ``-``.

``add`` uses the checkout containing the current directory as its source.
It creates a detached worktree by default. ``--detach`` and its alias ``-d``
request that behavior explicitly. ``-b`` creates a new branch.
``commit-ish`` defaults to ``HEAD`` and must resolve to the source's current
commit. ``--lock`` retains Git's worktree lock. ``--reason`` requires ``--lock``.
An omitted reason remains ``None`` in the returned metadata.

Use ``add --branch EXISTING_BRANCH PATH`` to attach an unused local branch
instead of creating one. It is mutually exclusive with ``-b`` and ``--detach``.
The branch must name the source commit. Git rejects a branch already checked out
elsewhere. The Rust equivalent is ``Branch::Existing``.
Neither failed creation nor removal deletes an existing branch. Index creation
never resets its reference, including when an external writer moves it.

Use ``add --committed PATH REF`` to fork a committed ref even when the caller's
checkout is dirty or at a different commit. With ``--branch``, that branch's tip
selects the commit. Omit REF. The Rust request uses
``SourceMode::Committed``. The default remains
``SourceMode::Checkout`` and preserves the clean-source requirement.

Committed mode resolves the ref and lets Git materialize the final destination.
It never stashes or resets the caller's working files or index. Git applies
checkout conversions and runs checkout hooks in the final worktree. No temporary
worktree or intermediate file clone is created.

Cowtree reserves tracked parent directories and uses Git's exclusive
``checkout-index`` operation. Colliding files or directories fail instead of
silently replacing one another on a case-insensitive filesystem. Committed
creation requires Git 2.36 or newer for native hook dispatch.

Before publication, Cowtree rejects staged changes and hidden index flags left
by hooks, then checks content against a fresh index. A hook cannot conceal
different tracked bytes through cached timestamps. This mode pays Git checkout
and verification costs and does not provide warm-cache inheritance. The existing
same-filesystem and native-volume admission rules still apply.

Clean-checkout mode populates tracked files through CoW. Both modes prepare a clean index.
There is no pass-through for arbitrary Git arguments. ``-B``, ``--force`` on
``add``, ``--orphan``, ``--cow``, ``--no-checkout``, tracking options, and
unknown options are rejected.

``list`` accepts a source checkout or defaults to the current repository.
Use ``--json`` for machine-readable output, including paths containing line
breaks. ``add``, ``doctor``, and ``remove`` also accept ``--json``: successes
contain ``status: "ok"``, ``kind``, and ``value``. Add returns the registered
worktree, doctor returns its probe report, and remove returns null. An unsupported
doctor probe has a null clone tool and still exits 1. Structured failures emit
one ``status: "error"``, ``code``, and ``message`` record on stderr. Invalid
syntax exits 2. ``list --json`` retains its existing array format.

``remove`` operates in the current repository and preserves branches.
``--force`` permits removal of a dirty worktree, but does not override a worktree
lock. Unlock it with ``git worktree unlock PATH`` first. ``doctor`` probes an
existing directory and defaults to the current directory.

Exit codes are ``0`` for success, ``1`` for an operation failure or unsupported
doctor result, and ``2`` for invalid command syntax.

``cowtree --version`` reports the compiled package version. Add ``--json`` for
``version`` and ``revision`` fields. Native builds embed ``COWTREE_BUILD_REVISION``
when the builder supplies it. Missing provenance is null ("unknown" in
text), never the current repository's HEAD.

Rust API
--------

The ``cowtree`` library in ``crates/libcowtree`` exposes::

    add_worktree(&AddRequest) -> WorktreeResult<Worktree>
    list_worktrees(Option<&Path>) -> WorktreeResult<Vec<Worktree>>
    remove_worktree(&Path, Option<&Path>, bool) -> WorktreeResult<()>
    inspect_path(&Path) -> WorktreeResult<DoctorReport>

``AddRequest::new(path)`` selects the current checkout, detached HEAD, and
``HEAD``. Set ``source`` to select another checkout. ``Branch::New`` creates a
branch and ``Branch::Existing`` attaches an unused branch. ``SourceMode::Committed``
materializes the requested ref. ``Lock::Retain`` retains the worktree lock and
its optional reason. ``Cancellation`` lets a caller request rollback between
native operations.

``Worktree`` contains the registered path, optional HEAD and branch, detached,
prunable, and locked flags, and an optional lock reason. Branch names are full
refs such as ``refs/heads/agent/task``. Paths use Git's registered spelling.
On filesystems that consider different Unicode spellings the same name,
Cowtree compares existing directories by filesystem identity during creation
and rollback. Missing unrelated registrations do not prevent another add.

``DoctorReport`` records the tested path, native mechanism, and refusal reason.
``supported()`` is true only when the clone-and-independent-write probe passes.
An unsupported volume returns a report. Invalid directories and operational
probe failures return typed errors.

Requirements
------------

In the default checkout mode, the source must be a normal, complete checkout
with a committed ``HEAD`` and no tracked changes. A linked worktree can serve
as the source. Sparse checkouts and index entries marked assume-unchanged or skip-worktree are
rejected. Untracked and ignored files do not affect eligibility and are not
copied. Tracked regular files, executable modes, and symlinks are supported.
Symlinks retain their original link text and normal Git checkout semantics.

Submodule handling is selected by ``--submodules``. The default leaves
uninitialized children empty and refuses initialized children. See
`standalone submodules <standalone-submodules.rst>`_ for independent pinned
repositories and cleanup rules.

The requested commit must equal the source's current commit unless committed
mode is selected. A branch requested with ``-b`` must not already exist.
``--branch`` instead requires an existing, unused branch. The destination
must not exist, even as an empty
directory or dangling symlink. Missing parent directories are created. Source
and destination must be on the same filesystem.

Native clone mechanisms are:

* macOS APFS: ``clonefile(2)``.
* Linux filesystems supporting reflinks: ``FICLONE`` directly, without ``cp``.
* Windows volumes with block refcounting, including ReFS:
  ``FSCTL_DUPLICATE_EXTENTS_TO_FILE``. Standalone commands use native byte-range
  locks. Git executable bits are retained in the index. Windows does not expose
  POSIX executable permissions. Real symlinks require Windows symlink privileges
  and a checkout configured to preserve them.

Windows exposes standalone commands only. Managed commands are available on
macOS and Linux. Their lock inheritance, process
supervision, and durability port is tracked separately. See
`Windows qualification <windows-native.rst>`_.

A real clone probe establishes support. Filesystem names alone are insufficient.
Cowtree has no ordinary-copy fallback. Run ``cowtree doctor DIRECTORY`` on the
intended mount. See `filesystem validation <filesystems.rst>`_ for the
qualification matrix.

Failures and concurrency
------------------------

Operations return ``WorktreeError``. Its ``code()`` method exposes the stable
CLI category, while the typed error retains its original cause.

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
     - The selected commit contains a submodule under the reject policy.
   * - ``submodule_initialized``
     - The default leave-uninitialized policy encountered an initialized source child.
   * - ``submodule_unavailable``
     - A pinned child lacks local objects or violates repository isolation requirements.
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
     - An add fails and its rollback also fails. Retained state requires inspection.

When an add fails after creating its destination, cowtree synchronously removes
its registration and destination, and deletes the new branch requested by that
add. A failed rollback raises ``cleanup_failed`` with the original failure and
cleanup context. Inspect ``git worktree list`` and the named path and branch
before retrying. Rollback also removes empty parent directories created by that
add. It preserves pre-existing directories.

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

The `versioned workspace design <workspace-protocol.rst>`_ explains the
immutable snapshots, path reservations, fencing tokens, and batched publication
used by managed workspaces. The `managed workspace guide
<managed-workspaces.rst>`_ defines the available CLI and Rust operations.
Transparent file-descriptor rebinding, native change tracking, and distributed
coordination remain future work. Bounded model checks do not establish those
capabilities or power-loss durability.

Contributing
------------

New external contributors must open a
`Contribution interest issue <https://github.com/windsornguyen/cowtree/issues/new?template=contribution.yml>`_
and receive a maintainer's vouch before opening a PR. See the
`contribution guide <../CONTRIBUTING.rst>`_ for the process, existing-contributor
access, and development checks. Bug reports and questions do not require a vouch.

Development
-----------

::

    cargo fmt --all --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    cargo test --workspace --all-targets --all-features
    cargo run -p xtask -- specs --cache /absolute/evidence/tlc

Native filesystem cases require a qualifying volume. Set
``COWTREE_EXPECT_SUPPORTED=1`` so missing clone support fails the run. The Linux
filesystem matrix also tests refusal on ext4 and XFS without reflinks.

Batched publication verification
---------------------------------

The `FencedPublish checks <../specs/FencedPublish.rst>`_ complement the workspace
model with captured proposal tokens, disjoint batches, and finite induction.
They validate a bounded protocol model and do not add worktree API operations.

Local metadata authority
------------------------

The Rust ``cowtree-metadata`` crate implements a single-host SQLite WAL
metadata authority with fenced path reservations, immutable snapshots, retry
receipts, and retention. Its library and JSON operation contracts are documented
in `crates/metadata/README.md <../crates/metadata/README.md>`_.
Run the complete publication example with::

    cargo run --locked -p cowtree-metadata --example publish

Standalone operations do not use this authority. ``cowtree_workspace::Workspace``
uses it in-process for publication and coordinates filesystem installation,
checkpoint retention, and recovery. Calling the metadata authority directly
changes only its logical view. Use the managed API for physical workspaces.
