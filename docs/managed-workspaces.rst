Managed workspaces
==================

A managed workspace combines a Rust SQLite publication authority, immutable
private snapshots, and ordinary Git worktrees populated through native
copy-on-write (CoW). A leaf is one mutable working directory. A checkpoint is
a retained private snapshot. Publishing advances the shared source tip only
after a command has checked the exact candidate in an isolated working directory.

Build and start
---------------

Use macOS APFS or Linux Btrfs/XFS with reflinks enabled. Cowtree probes native
clone support and fails on unsupported filesystems. The workspace store, source,
and leaves must share a filesystem. Keep the store outside every checkout.
Install Rust and Git, then run from this repository::

    cargo build --locked --release -p cowtree-metadata
    uv sync --locked
    uv run cowtree workspace --root /absolute/store init \
        --source /absolute/project \
        --binary /absolute/cowtree/target/release/cowtree-metadata \
        --derived build --ephemeral .env
    uv run cowtree workspace --root /absolute/store fork /absolute/writer

The returned JSON contains the leaf ``id``. The store records the exact executable
path; it does not silently choose a different backend. Git author identity must
be configured because snapshots create source commits. A managed leaf remains
detached and locked in Git; use the managed commands for its lifecycle.

``cowtree workspace --root STORE version`` reports the CLI and configured
metadata executable separately without opening the authority store. The backend
supports ``cowtree-metadata --version --json``; older binaries reject this query
without creating a store. Build with ``COWTREE_BUILD_REVISION`` set to a known
source identity to include it. An unstamped binary reports a null revision.

Source and caches
-----------------

Source includes tracked files, dependency lockfiles, and non-ignored untracked
files. ``--derived PREFIX`` selects a cache subtree. ``--ephemeral PREFIX`` selects
files to exclude. Repeat either option for several prefixes. Unselected ignored
files, Git control state, and Cowtree control state are excluded. Cowtree does not
infer secrets from file contents: declare private runtime paths explicitly.

For Cargo targets with hard links, ``--derived-hardlinks clone`` explicitly
clones each derived pathname to a separate inode. The default ``reject`` policy
refuses those inputs. Source hard links remain unsupported. See
`Cargo workspace qualification <cargo-workspaces.rst>`_.

Derived symlinks must use relative targets that resolve within selected cache
prefixes. Absolute links and escaping or cyclic links are rejected. A ``target``
symlink to an external compiler cache would otherwise let every leaf write into
the same directory. Select a real private cache directory for inheritance.

Tracked source cannot be reclassified as derived or ephemeral. Overlapping policy
prefixes fail. Forks inherit eligible cache bytes, modes, and modification times
through private CoW copies; cache writes remain isolated. Source publication
does not make another leaf's cache authoritative source. Successful check output
can warm future forks from that published lineage.

Compiler reuse depends on the build system, path assumptions, and inputs.
The `make/Clang experiment <warm-cache.rst>`_ measures actual invocations as well
as elapsed time. It shows reuse without claiming every cache is portable.

Edit, check, publish
--------------------

Suppose the fork returned leaf 2. Edit its files normally, then run::

    cowtree workspace --root /absolute/store capture 2
    cowtree workspace --root /absolute/store prepare 2
    cowtree workspace --root /absolute/store check 2 --timeout 300 -- make test
    cowtree workspace --root /absolute/store commit 2

``capture`` freezes source intent and acquires the necessary path reservations.
It returns null if source has not changed. It preserves subsequent working edits.
``prepare`` binds the request to one parent, attempt, and source root.
``check`` runs an argv command, without a shell, against that exact candidate.
The command must exit successfully and leave candidate source unchanged.
``commit`` refuses an unchecked candidate. Repreparing invalidates old checks.

The check runs under a deadline and bounded log policy. Commands must remain
within their supervised process group and must not modify workspace metadata.
This is cooperative process management, not a sandbox for untrusted commands.

After publishing, synchronize another leaf with ``sync LEAF``. Dirty paths and
submitted paths stay intact. Edits made after capture remain private when its
receipt is acknowledged. Git projection records only source and retains the
exact checked commit identity. It does not move a caller-owned Git branch.

The Python API returns the exact ``Candidate`` so callers can retain it for
idempotent commit retries. From the CLI, save the request's leaf and sequence
from ``capture``; use ``result LEAF SEQUENCE`` after an uncertain commit reply.
A new capture is a new request, not a retry of an old request.

Checkpoints and collection
--------------------------

``seal LEAF`` freezes source and eligible caches into a private checkpoint and
retains it. ``fork PATH --node NODE`` creates another leaf from that checkpoint.
``retain NODE`` and ``release NODE`` control manual retention. ``log`` lists the
currently retained physical nodes and their history identities. Equal content
can have different history IDs; collection may remove unreferenced ancestors.

``drop LEAF`` refuses unpublished source changes. ``drop LEAF --force`` explicitly
discards that working directory. ``collect`` removes unreferenced private images,
retired check worktrees and bounded artifacts, then runs authority maintenance.
Live leaves, retained checkpoints, pending candidates and active clone sources
stay pinned. Durable client object pins protect old comparison origins even when
their authority epoch has aged out. See `Collection <collection.rst>`_.

Conflicts and batches
---------------------

Reservations include path prefixes. Two leaves cannot publish conflicting paths
as a disjoint batch. A stale dirty origin requires an explicit resolution.
Abort a pending capture first; abort retires its request while preserving files.
Then pass a JSON file mapping each chosen path to ``local`` or ``published``::

    cowtree workspace --root /absolute/store resolve 2 --choices choices.json

For example, ``{"src/main.py":"local"}`` keeps a manual merge already written
in that file and updates its comparison origin under a fresh reservation.
``published`` installs the granted source value. Neither choice publishes it.
Another live owner must release its reservation or be deliberately retired;
resolution does not steal a live lease.

For disjoint writers, capture each one, then prepare their combined candidate::

    cowtree workspace --root /absolute/store prepare-batch 2 3 > response.json
    jq '.value' response.json > candidate.json
    cowtree workspace --root /absolute/store check-batch \
        --candidate candidate.json --timeout 300 -- make test
    cowtree workspace --root /absolute/store commit-batch --candidate candidate.json

Preserve ``candidate.json`` for retries. Checks run once on the complete union.
All members commit in one epoch or none do. Changing membership or repreparing
any member requires a new check. See `Checked batches <batch-publication.rst>`_.

Supported interface
-------------------

Every managed CLI command begins ``cowtree workspace --root STORE``.
``--help`` describes its exact arguments. ``git cowtree`` has the same interface.

.. list-table:: CLI to Python API
   :header-rows: 1
   :widths: 35 65

   * - CLI
     - Python owner and operation
   * - ``init``
     - ``cowtree.workspace.Workspace.create``; ``Workspace.open`` reopens a store.
   * - ``import-status``
     - ``Workspace.import_status`` returns the durable initial-import cursor.
   * - ``list``, ``fork``
     - ``cowtree.leaves.Leaves.records``, ``Leaves.fork``.
   * - ``acquire``, ``sync``, ``discard``
     - ``cowtree.views.Views.acquire``, ``Views.sync``, ``Views.discard``.
   * - ``seal``, ``retain``, ``release``
     - ``cowtree.seals.Seals.seal``, ``Seals.retain``, ``Seals.release``.
   * - ``capture``, ``abort``
     - ``cowtree.captures.Captures.capture``, ``Captures.abort``.
   * - ``prepare``, ``commit``, ``result``
     - ``cowtree.publications.Publications.prepare``, ``commit``, ``result``.
   * - ``check``
     - ``cowtree.checks.Checks.validate``.
   * - ``prepare-batch``, ``check-batch``, ``commit-batch``
     - ``cowtree.batches.Batches.prepare``, ``validate``, ``commit``.
   * - ``resolve``
     - ``cowtree.resolutions.Resolutions.resolve`` with ``Choice`` values.
   * - ``drop``, ``collect``
     - ``cowtree.lifecycle.Lifecycle.drop``, ``cowtree.collection.Collector.collect``.
   * - ``recover``
     - ``Workspace.recover_initialization`` or a recovering ``Workspace.session``.
   * - ``log``
     - Read node records under ``Workspace.session`` through ``Workspace.nodes``.

Construct each service with the opened ``Workspace``. Read records under a session
when several observations must be consistent. Service mutations open their own
sessions; do not nest them inside an existing session. The original ``cowtree.core``
API remains for independent tracked-file worktrees.

``discard LEAF PATH...`` restores each selected path to its submitted capture,
if one exists, or its last installed origin. It preserves the pending request
and unrelated edits. It does not fetch the newest shared tip.

Large initial imports run in bounded, resumable chunks. Use ``import-status``
to observe durable progress and ``recover`` to resume the same captured snapshot.
See `Initial import <initial-import.rst>`_ and the
`BSMR materialization boundary <bsmr-workspaces.rst>`_.

Failures and recovery
---------------------

Managed successes emit one JSON object on stdout with ``status``, ``kind`` and
``value``. Failures emit one JSON object on stderr with ``status: "error"``,
``code`` and diagnostic ``message``. Authority failures also carry typed
``details`` and ``retry_action``. Exit status is 0 for success, 1 for an operation
failure, and 2 for invalid usage. Help is text. Never parse diagnostic wording.

Authority actions are ``none``, ``retry_same_request``, ``reprepare``,
``run_maintenance``, or ``resolve_conflict``. Only SQLite BUSY/LOCKED and a blocked
checkpoint receive same-request retry guidance. Corrupt objects, stale authority,
invalid paths and schema failures do not become transient retries.

Normal sessions recover pending client journals before admitting mutations.
``recover`` resumes interrupted initialization, fork, installation, checkpoint,
publication acknowledgement, drop and quarantine cleanup. It refuses changed
directory identities and intervening edits rather than overwriting them.
An interrupted initialization before claiming its staging name can leave a private
temporary sibling; it cannot block retry and is not broadly deleted by recovery.

A failure may arrive after durable work completed. Query ``result`` or reopen
and recover before deciding to submit a new request. The authority schema and
workspace layout must match the current release. Incompatible stores fail without
schema or data migration. See `Declarative schema <schema.rst>`_.

Use the managed API exclusively for its store. Calling raw authority mutations
or editing client records bypasses its filesystem lock and is outside this contract.
Standalone Rust metadata users have their own explicit protocol documented in
the `crate reference <../crates/metadata/README.md>`_.

Filesystem boundary
-------------------

Capture, installation and checkpointing require quiescent writers for that leaf.
Readers that require a consistent whole tree must also coordinate with installation:
replacement is journaled per path, not one atomic directory switch. Existing open
descriptors keep the old inode; new path lookups see the replacement. Raw mmap or
file-descriptor writers are not transparently rebound.

The portable source namespace uses UTF-8 NFC names and rejects case/Unicode aliases,
non-directory prefix conflicts, reserved control names, hard links and special
files. Symlink contents and executable mode are preserved; parent symlinks are
not traversed. Rename is captured as deletion plus insertion. Empty directories
may exist privately but are not published source entries. Sparse checkouts are
unsupported. Submodules are rejected by default. Explicit
`pinned materialization <pinned-submodules.rst>`_ admits clean direct dependencies
as read-only source in the private projection, without nested Git control state.

Qualification
-------------

Run from a source checkout on a supported filesystem::

    cargo test --locked --workspace --all-targets --all-features
    cargo build --locked -p cowtree-metadata
    uv run pytest src tests mounted integration
    uv run python scripts/check_specs.py --cache /absolute/tlc-cache
    PYTHONPATH=src uv run python benchmarks/workspace_history.py --help
    PYTHONPATH=src uv run python benchmarks/warm_cache.py --help

The Hollywood-generated CI runs the mounted suite on APFS and Btrfs/reflink-XFS,
plus explicit refusal tests on ext4 and XFS without reflinks. `Verification
<verification.rst>`_ identifies exact bounded models and runtime trace mappings.
`Lifecycle qualification <workspace-qualification.rst>`_ records seeded histories.
Process-exit tests do not establish power-loss durability or distributed consensus.

Future native/distributed filesystems, transparent writer rebinding, native change
tracking and unbounded Lean/Veil proofs remain separate work. The
`design reference <agent-filesystem.rst>`_ preserves those goals; this guide defines
the local contract callers can exercise today.
