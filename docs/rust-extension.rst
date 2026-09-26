Native engine
=============

Cowtree runs as a Rust executable. It calls Git for refs, indexes, checkout
conversions, and worktree registration. The filesystem shares unchanged file
blocks and isolates later writes. Ordinary edits do not run a Cowtree watcher
or create a checkpoint.

The CLI and Rust APIs use the same native operations. Managed workflows use
one in-process SQLite connection per workspace session. A separate native
supervisor bounds validation commands and retains their ownership lock if
the coordinating process exits.

Ownership
---------

.. list-table:: Crates
   :header-rows: 1

   * - Directory
     - Responsibility
   * - ``crates/libcowtree``
     - Native cloning, tree capture, standalone Git worktrees, cancellation.
   * - ``crates/git``
     - Synchronous Git execution, command context, pipes, and explicit lock inheritance.
   * - ``crates/workspace``
     - Managed imports, warm forks, checkpoints, publication, recovery, collection.
   * - ``crates/metadata``
     - SQLite authority, fenced reservations, immutable objects, publication receipts.
   * - ``crates/process``
     - Validation deadlines, log limits, process groups, inherited locks.
   * - ``crates/cli``
     - Argument parsing and JSON output for ``cowtree`` and ``git-cowtree``.
   * - ``crates/xtask``
     - Development-only schema generation and protocol-model checking.

Each library has an index-only ``lib.rs``. Named modules own behavior. Rust
errors retain their filesystem, process, Git, or authority cause until the CLI
formats a diagnostic. Python and PyO3 are not part of the build or runtime.

Committed standalone creation checks out directly into its final destination.
Checkout-based creation uses CoW clones of existing files. Both modes share
one ownership and rollback transaction. Managed capture keeps one workspace
session across its phases. Checkpoint publication has one owner, while recovery
verifies persisted images before completing their journaled transitions.

The managed store retains the executable path recorded at initialization as
provenance. Validation uses the current caller's explicit supervisor executable.
The CLI supplies its own path. It does not launch a formerly configured metadata
service or interpreter when reopening an existing store.

Build and verify
----------------

From the repository root::

    cargo build --locked --release -p cowtree-cli
    cargo fmt --all --check
    cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
    cargo test --locked --workspace --all-targets --all-features
    cargo doc --locked --workspace --no-deps

Standalone native cloning supports macOS, Linux, and Windows on qualifying
filesystems. Managed filesystem ownership and recovery support macOS and Linux.
The managed Windows port remains separate work.

Performance
-----------

Measure complete creation, including startup, admission, Git calls, cloning,
and content verification::

    cargo run --release -p cowtree-cli --example benchmark -- \
        --binary target/release/cowtree --files 8192 --trials 5 \
        --output /tmp/cowtree-benchmark.json

The benchmark alternates Git and Cowtree, checks every destination's bytes and
Git status, and removes only its owned worktrees. It reports raw wall times.
Logical fixture bytes are not physical allocation measurements.

Add ``--source-mode committed`` to measure direct committed materialization.
The default ``--source-mode checkout`` measures CoW cloning of the existing
checkout. Compare each mode with Git on the same fixture and build.

Standalone population creates each parent once and uses at most four clone
workers. Every worker finishes before success or rollback can proceed.
Cancellation is a shared atomic request checked between files. Native clone
primitives create destinations exclusively, so an existence probe is unnecessary.
The opened source descriptor is still validated before cloning.

See `native profiling <native-profiling.rst>`_ for debugger and syscall evidence.
Space savings, creation latency, and compiler cache reuse are separate results.
