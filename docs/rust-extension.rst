Native engine
=============

An agent changes one file in a checkout containing thousands. The filesystem
allocates private storage for that write. The other worktrees keep their original
bytes. Cowtree does not run a watcher or rewrite a private block map on each save.

Native clone APIs establish that sharing. Ordinary writes, truncation, atomic
saves, and deletion then use the filesystem's own copy-on-write rules. This does
not create a Cowtree checkpoint. Checkpointing remains an explicit managed
operation, and file durability still depends on the caller's writes and syncs.

Why Rust
--------

The kernel handles sharing for one file. Cowtree must still validate a repository,
walk many paths, clone each file, preserve metadata, build the index, and undo
owned changes if any step fails. ``libcowtree`` owns that complete standalone
operation, so its Rust CLI and Python bindings cannot drift into different rules.

Rust also owns snapshot traversal, hashing, policy checks, and materialization.
The library does not reimplement Git or the filesystem. Git owns refs, indexes,
and worktree registration. The kernel owns shared blocks and independent writes.

Interfaces
----------

``crates/libcowtree``
    Required native engine. Typed requests select source, branch, and lock
    policies. Errors retain the failed operation and operating-system cause.
``crates/cli``
    Native ``cowtree`` and ``git-cowtree`` executables for standalone commands.
    Neither embeds Python nor starts a Python process.
``crates/cowtree-python``
    PyO3 binding to the same engine, with the stable CPython 3.10 ABI.
    Long operations release Python's interpreter lock. Creation checks Python
    signals at cancellation points before returning or rolling back.
``src/cowtree/core.py`` and ``src/cowtree/trees.py``
    Typed Python records and native error translation. A missing extension is
    an installation failure, not a request to select another implementation.

Managed publication and crash recovery still have a Python coordinator under
``src/cowtree`` and a Rust SQLite authority in ``crates/metadata``.
The native CLI does not expose ``workspace`` commands yet. Use the Python
compatibility command for them. Moving that coordinator into ``libcowtree`` is
remaining work, not an optional acceleration mode.

Build and test
--------------

From the repository root::

    cargo build --locked --release -p cowtree-cli
    ./target/release/cowtree doctor .
    cargo test -p libcowtree
    uv sync --group dev
    uv run pytest -q
    uv build

Run the existing command-contract tests against the native executable::

    COWTREE_TEST_BINARY="$PWD/target/release/cowtree" uv run pytest -q \
        tests/test_cli.py tests/test_cli_json.py \
        tests/test_existing_branch.py tests/test_committed_ref.py

Hatch builds the Python package and strips inline tests from release artifacts.
Maturin compiles the extension. Editable installs retain the source package.
Source archives explicitly include build inputs and exclude local binaries.

Performance and qualification
-----------------------------

Use the same fixture for Git, Python bindings, and the native executable::

    uv run python benchmarks/native.py --binary target/release/cowtree \
        --files 512 --worktrees 4 --trials 3 --output /tmp/cowtree-native.json

The harness rotates execution order and verifies every destination with Git.
Creation time includes validation and registration, not only the clone syscall.
Space savings do not imply lower latency. Git subprocesses and filesystem
metadata can dominate even after the Python per-file loop is removed.

The `native qualification run at dfd1dda
<https://github.com/windsornguyen/cowtree/actions/runs/35514459984>`_ passes the
Linux filesystem matrix and Python 3.10-3.14 tests on Linux and macOS. On each
Windows architecture, it passes 23 Python-boundary tests, 18 Rust engine tests,
and 12 native-executable tests. The Windows hosts are x64 Server 2025/ReFS and
ARM64 Windows 11/Dev Drive.

Those Windows results qualify standalone commands. They do not establish
managed-workspace ownership, process supervision, or durability on Windows.
The managed coordinator remains POSIX-only until that separate port is qualified.

References
----------

* `Apple clonefile contract <https://github.com/apple-oss-distributions/xnu/blob/main/bsd/man/man2/clonefile.2>`_
* `PyO3 packaging <https://pyo3.rs/v0.29.0/building-and-distribution.html>`_
* `PyO3 signal handling <https://pyo3.rs/v0.29.0/faq.html#ctrl-c-doesnt-do-anything-while-my-rust-code-is-executing>`_
* `Hatch build hooks <https://hatch.pypa.io/latest/plugins/build-hook/reference/>`_
