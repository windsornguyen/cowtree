Local workspace initialization
===============================

``Workspace::create(&CreateRequest)`` imports a clean Git checkout
into a local SQLite authority and an immutable warm snapshot. The store must
be absent, outside the source checkout, on a native CoW filesystem. Select the
compiled metadata executable explicitly. The source writer remains quiescent
until capture completes.

The source-only initial snapshot uses a bounded import available only on a
pristine authority. Progress commits per chunk; epoch one appears only after
all source objects are durable. No temporary leaf or per-file view is allocated.
Declared caches remain in the private snapshot and ignored files are ephemeral
unless selected as caches. Git objects reuse the existing common directory.
The original checkout, index, and branch remain unchanged.

Initialization uses a sibling staging directory and exclusive rename. The
workspace becomes visible only after its data and configuration are durable.
``Workspace::recover_initialization`` resumes a completed immutable capture from
its durable import cursor and publishes the completed store. Before capture or
the import record is complete, it removes only the owned unacknowledged staging
store and matching private Git refs. An existing workspace is an idempotent
recovery result. Unexpected destination contents are preserved.

The workspace's cooperative lock covers filesystem transitions. Direct mutation
of its owned metadata or object store is unsupported. Process interruption tests
do not establish power-loss durability. See `Initial import <initial-import.rst>`_
for progress, throughput measurements, and the independent byte oracle.

Build and test with::

    cargo build --locked -p cowtree-metadata
    cargo test -p cowtree-cli --all-features failed_tree_flush

The mounted profile requires native CoW rather than skipping. CI runs it on
APFS and the supported btrfs/XFS matrix mounts. The ordinary Python and metadata
profiles remain separate. Workflow sources accept dependent PR bases so each
stack layer receives its own checks.
