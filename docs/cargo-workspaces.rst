Cargo workspaces
================

A managed fork carries source and a private copy-on-write view of selected build
outputs. Each agent can edit and build without sharing writable output files with
another agent. Cargo still decides whether an artifact's inputs match.

First workspace
---------------

Build the native executable, then warm the source project before initialization::

    cargo install --locked --path crates/cli
    cd /absolute/project
    cargo build
    cowtree doctor /absolute/worktrees
    cowtree workspace --root /absolute/store init \
        --source /absolute/project --derived target
    cowtree workspace --root /absolute/store fork /absolute/worktrees/agent

The store and forks must be outside the source checkout and on the same qualifying
filesystem. Source must have a clean tracked checkout. ``target`` may contain
ignored outputs. Initialization freezes the admitted source and selected cache.

Edit and publish from a private leaf::

    cd /absolute/worktrees/agent
    # Edit source, then capture the intended change.
    cowtree workspace --root /absolute/store capture 1
    cowtree workspace --root /absolute/store prepare 1
    cowtree workspace --root /absolute/store check 1 -- cargo test
    cowtree workspace --root /absolute/store commit 1

Use the returned leaf identity rather than assuming it is 1. Validation runs in
a separate warm worktree containing the exact prepared candidate. A successful
check retains its warmed cache for future forks. Edits made in the writer after
capture remain private and are not included in that publication.

A validation command may use Rust, Python, or another project tool. Cowtree itself
requires neither an interpreter nor a separately installed metadata executable.
The native supervisor enforces the validation deadline and retains its lock if
the coordinating Cowtree process exits.

Derived targets
---------------

Declare the directories the project actually uses. If Cargo sets ``CARGO_TARGET_DIR``
or a build directory outside ``target``, select the corresponding in-checkout
prefix explicitly. Cowtree does not infer compiler cache validity or silently
include arbitrary ignored directories. Ephemeral prefixes are omitted.

Source manifests reject hard links. Derived aliases require the explicit
``--derived-hardlinks clone`` policy, which makes each cache pathname independent.
Derived symlinks must remain within the selected private cache namespace.

Use ``seal`` to retain an unpublished source/cache checkpoint, and ``fork --node``
to create descendants from it. ``release`` removes a manual retention pin.
``collect`` preserves live leaves, pending operations, validations, and retained
checkpoints before reclaiming unreferenced images.

Qualification
-------------

::

    COWTREE_EXPECT_SUPPORTED=1 cargo test --workspace --all-features
    cargo run -p xtask -- specs --cache /absolute/evidence/tlc

The CLI tests cover source and cache isolation, exact-candidate validation,
post-capture edits, batch publication, checkpoint descendants, and recovery.
A project's real compiler outputs still need project-specific validation.
See `performance <performance.rst>`_ for historical compiler-reuse evidence and
its environment boundaries.
