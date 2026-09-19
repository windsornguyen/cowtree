Local workspace initialization
===============================

``Workspace.create(root, source, binary, policy)`` imports a clean Git checkout
into a local SQLite authority and an immutable warm snapshot. The store must
be absent, outside the source checkout, on a native CoW filesystem. Select the
compiled metadata executable explicitly. The source writer remains quiescent
until capture completes.

The source-only initial snapshot is committed through the existing fenced
publication operations. The temporary importing leaf is dropped afterward.
Declared caches remain in the private snapshot and ignored files are ephemeral
unless selected as caches. Git objects reuse the existing common directory.
The original checkout, index, and branch remain unchanged.

Initialization uses a sibling staging directory and exclusive rename. The
workspace becomes visible only after its data and configuration are durable.
``Workspace.recover_initialization`` publishes a completed staging store or
aborts an incomplete, unacknowledged import. Abort removes only the owned staging
store and its matching private Git refs. An existing workspace is an idempotent
recovery result. Unexpected destination contents are preserved.

The workspace's cooperative lock covers filesystem transitions. Direct mutation
of its owned metadata or object store is unsupported. Process interruption tests
do not establish power-loss durability. Source import currently stages complete
file objects and has not been qualified for large-repository throughput.

Build and test with::

    cargo build --locked -p cowtree-metadata
    uv run pytest -q mounted/test_bootstrap.py

The mounted profile requires native CoW rather than skipping. CI runs it on
APFS and the supported btrfs/XFS matrix mounts. The ordinary Python and metadata
profiles remain separate. Workflow sources accept dependent PR bases so each
stack layer receives its own checks.
