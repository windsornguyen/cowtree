Standalone macOS installation
=============================

The macOS arm64 bundle contains the Cowtree CLI, its Python interpreter and
runtime dependencies, and the release Rust metadata executable. Git must be
installed separately. Managed workspaces require native copy-on-write support;
on macOS, use an APFS volume.

Install the whole directory
---------------------------

Extract the supplied archive to its final location. Keep ``cowtree``,
``cowtree-metadata``, and ``_internal`` together; moving just the executable
breaks the bundle. The archive preserves the runtime's symbolic links::

    mkdir -p "$HOME/.local/opt"
    tar -xzf cowtree-macos-arm64.tar.gz -C "$HOME/.local/opt"
    export COWTREE_BIN="$HOME/.local/opt/cowtree/cowtree"
    export COWTREE_METADATA="$HOME/.local/opt/cowtree/cowtree-metadata"
    "$COWTREE_BIN" --help
    "$COWTREE_BIN" doctor /path/to/apfs/workspaces

The local build uses an ad-hoc signature, not Developer ID notarization. Check
the supplied SHA-256 before running a transferred archive::

    shasum -a 256 -c cowtree-macos-arm64.tar.gz.sha256

Initialize once beside a clean repository, with build processes stopped::

    "$COWTREE_BIN" workspace --root /path/to/apfs/store init \
      --source /path/to/repository \
      --binary "$COWTREE_METADATA" \
      --derived target --derived-hardlinks clone
    "$COWTREE_BIN" workspace --root /path/to/apfs/store fork /path/to/apfs/leaf

The metadata executable's absolute path is saved in the workspace. Install the
bundle at its final location before initialization and keep that location while
using the store. ``--derived-hardlinks clone`` explicitly expands hard-linked
Cargo cache paths into independent copy-on-write files; source hard links remain
unsupported. Cowtree does not make cached compiler outputs portable across
platforms, toolchains, or changed absolute build paths.

See `Cargo workspaces <cargo-workspaces.rst>`_ for a complete build and publication
walkthrough. Cowtree's own runtime needs no Python or Rust installation; a Cargo
build still needs the user's selected Rust toolchain.

Build and verify
----------------

Developers need ``uv``, a Rust toolchain, Git, and Apple's command-line tools.
Run from the repository root on Apple silicon macOS::

    uv run --no-project --python 3.14 scripts/package.py
    COWTREE_BUNDLE="$PWD/dist/cowtree" uv run pytest -q mounted/test_package.py
    tar -czf dist/cowtree-macos-arm64.tar.gz -C dist cowtree
    cd dist
    shasum -a 256 cowtree-macos-arm64.tar.gz > cowtree-macos-arm64.tar.gz.sha256

``scripts/package.py`` refuses an existing output bundle. It builds a wheel using
the repository's inline-test stripping hook, rejects remaining inline-test
imports, and freezes that installed wheel using PyInstaller 6.22.3. It builds
the metadata executable with ``cargo build --locked --release``. Temporary build
environments remain under ignored ``build/``; deliverables remain under ignored
``dist/``. Use ``--dist /another/absent/output`` for another build.

The smoke test copies the complete bundle to a different directory and runs the
public workspace lifecycle outside the checkout. Its PATH exposes Git alone.
It checks successful publication, failed commands, timeout handling, source/cache
bytes, and preservation of the user's child environment. Candidate validation
uses the embedded supervisor through the frozen executable; source execution
continues to use Python's isolated interpreter mode.

The bundle targets macOS arm64. It is not a Linux or Windows binary, and the
smoke test is process-level qualification rather than sudden power-loss evidence.
