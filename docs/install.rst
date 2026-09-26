Native installation
===================

Build Cowtree with Rust 1.85 or later and a C toolchain. Git 2.36+ must be on ``PATH``.
The installed commands use native code and do not require a Python interpreter::

    cargo install --locked --path crates/cli
    cowtree --version --json
    cowtree doctor /path/to/worktrees

Cargo installs ``cowtree`` and ``git-cowtree``. Git finds the latter as
``git cowtree`` when Cargo's binary directory is on ``PATH``. The selected Git
may be upstream Git or a compatible distribution such as OpenAI Git.

Create a standalone worktree from a clean source checkout::

    cowtree add -b agent/task ../worktrees/task

For managed source, build caches, and checked publication::

    cowtree workspace --root /absolute/store init \
        --source /absolute/project --derived target
    cowtree workspace --root /absolute/store fork /absolute/worktrees/task

The source, store, and worktrees must share a filesystem with native cloning.
Use APFS on macOS or a qualifying reflink filesystem on Linux. Windows supports
standalone cloning on qualifying volumes such as ReFS. Managed Windows
ownership, supervision, and durability are not implemented.

For an uninstalled development build::

    cargo build --locked --release -p cowtree-cli
    ./target/release/cowtree doctor .
    COWTREE_EXPECT_SUPPORTED=1 cargo test --workspace --all-features

Set ``COWTREE_BUILD_REVISION`` during compilation to embed an immutable source
revision. An omitted revision is reported as null. It is never inferred from
whichever repository the caller happens to be in.
