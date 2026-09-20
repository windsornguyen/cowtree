Cargo workspaces
================

Cowtree owns private filesystem views. BSMR remains the action-cache authority:
it decides which inputs identify a build and which cached outputs are reusable.
An inherited Cargo target directory is disposable derived state. Its presence
does not establish a cache hit or make an artifact portable to another compiler,
platform, environment, or absolute checkout or registry-source path.

First workspace
---------------

An intern can start a build as soon as a prepared leaf is handed over. Prepare
that leaf before handoff, assign its path to one person, and keep using the same
leaf for edits and builds. Repeated builds do not require another fork. A cold
import and a new fork still take time; large Rust repositories do not have a
sub-second guarantee.

Install Git and an appropriate Rust toolchain. Use a native-clone filesystem:
APFS on macOS, or Btrfs/reflink-enabled XFS on Linux. The source, store, and leaves
must share a filesystem. The store and leaves must sit outside the source checkout.

1. Follow the `binary installation guide <install.rst>`_ and keep the extracted
   directory in a stable location. Select the two sibling executables::

       export COWTREE_BIN=/absolute/tools/cowtree/cowtree
       export COWTREE_METADATA=/absolute/tools/cowtree/cowtree-metadata

   For a developer installation instead, install uv and build both components from
   the same reviewed revision::

       git clone https://github.com/windsornguyen/cowtree.git /absolute/tools/cowtree
       cd /absolute/tools/cowtree
       : "${COWTREE_REV:?Set COWTREE_REV to the reviewed commit hash}"
       git checkout --detach "$COWTREE_REV"
       export COWTREE_CODE="$PWD"
       CARGO_TARGET_DIR="$COWTREE_CODE/target" \
         CARGO_BUILD_BUILD_DIR="$COWTREE_CODE/target" \
         cargo build --locked --release -p cowtree-metadata
       uv sync --locked
       export COWTREE_BIN="$COWTREE_CODE/.venv/bin/cowtree"
       export COWTREE_METADATA="$COWTREE_CODE/target/release/cowtree-metadata"

   When using an existing local checkout, verify ``git rev-parse HEAD`` and build
   from the selected revision. Keep the installation directory in place while
   its stores exist; each store records the metadata executable's absolute path.
   Select the reviewed commit explicitly. Test a newer tip before replacing an
   existing store's executable; incompatible prerelease layouts require a new store.

2. Choose existing source and absent destination paths. Configure a Git author
   identity if ``git var GIT_AUTHOR_IDENT`` fails. Stop editors, watchers, and build
   processes that write to the source before importing it. Warm the source with
   the toolchain and Cargo home that the intern will also use::

       export PROJECT=/absolute/cow-volume/project
       export STORE=/absolute/cow-volume/project-store
       export LEAF=/absolute/cow-volume/intern-work
       export CARGO_HOME=/absolute/existing/cargo-home
       cd "$PROJECT"
       export CARGO_BIN="$(rustup which cargo)"
       export RUSTC="$(rustup which rustc)"
       export RUSTC_WRAPPER= RUSTC_WORKSPACE_WRAPPER=
       CARGO_TARGET_DIR="$PROJECT/target" CARGO_BUILD_BUILD_DIR="$PROJECT/target" \
         "$CARGO_BIN" build --locked

   Select a Cargo home that already contains this project's dependencies. Keep its
   absolute path, compiler, profile, features, and build environment stable across
   the source and leaf. Relocating registry sources can invalidate Cargo's cache
   fingerprints. The commands disable compiler wrappers for this direct Cargo
   workflow and place all generated build files under the selected target directory.

3. Import the quiescent source once, then prepare the intern's leaf::

       "$COWTREE_BIN" doctor "$PROJECT"
       "$COWTREE_BIN" workspace --root "$STORE" init \
         --source "$PROJECT" --binary "$COWTREE_METADATA" \
         --derived target --derived-hardlinks clone --ephemeral .env
       "$COWTREE_BIN" workspace --root "$STORE" fork "$LEAF"

   The fork returns JSON containing its leaf ``id``. Save it for lifecycle commands.
   The explicit hard-link policy gives every inherited Cargo output pathname its
   own inode. The default policy rejects hard links. Secrets outside ``.env`` need
   their own ephemeral prefixes. Tracked inputs cannot be declared ephemeral.

4. Hand the intern the completed path and the same build environment. Compile,
   edit a source file, and compile again in that leaf::

       cd "$LEAF"
       export CARGO_TARGET_DIR="$LEAF/target"
       export CARGO_BUILD_BUILD_DIR="$LEAF/target"
       "$CARGO_BIN" build --locked --offline --message-format=json > ../intern-first-build.jsonl
       # Edit a source file in this leaf, then run the same command again.
       "$CARGO_BIN" build --locked --offline --message-format=json > ../intern-edited-build.jsonl

   Cargo reports each compiler artifact's ``fresh`` value in those JSON lines.
   Functional build/run results establish whether the inherited outputs work.
   Edits and cache writes stay private until explicit source publication. See the
   `capture, check, and publish procedure <managed-workspaces.rst#edit-check-publish>`_
   when the change is ready. Stop the leaf's writers before capture, installation,
   or recovery.

Each prepared path needs one owner. Cowtree's ``acquire`` command reserves source
paths for publication; it does not assign a checkout to a worker. A caller that
offers a pool of prepared leaves must provide its own exclusive assignment.

Runnable smoke test
-------------------

The CLI smoke test creates a disposable two-crate workspace with no downloaded
dependencies. It warms the seed target, imports it, prepares a leaf, builds twice,
edits the dependency, and builds again. It verifies the unchanged source/cache and
cleans up its own leaf. From the checkout containing this guide and smoke test,
run it with the release metadata executable::

    uv sync --locked --group test
    COWTREE_QUICKSTART_CLI="$COWTREE_BIN" \
    COWTREE_QUICKSTART_BINARY="$COWTREE_METADATA" \
      PYTHONPATH=src uv run --no-sync pytest mounted/test_cargo_quickstart.py -q -s

The printed receipt separates import and fork latency from compilation. It also
records fresh and compiled artifact counts. It checks that an unchanged repeated
build compiles zero artifacts and that the dependency edit produces the changed
program output without changing the source checkout.

Derived targets
---------------

Select the target directory explicitly with ``PathPolicy(derived=("target",))``.
Source files and Cargo.lock remain source inputs. Build processes must stop before
capture, installation, or recovery; open file descriptors and memory maps can
continue to reference old inodes during per-path installation.

The default policy rejects hard-linked files in every class. Cargo can create
hard links between a final executable and its dependency artifact. Callers that
accept independent cache paths can select
``derived_hardlinks=DerivedHardlinks.CLONE``. The enum lives in
``cowtree.tree_types``. Each derived pathname becomes a separate native
copy-on-write clone with the original bytes, mode, and modification time.
Mutating one cloned name does not update another formerly linked name. This is
an explicit change to cache alias semantics, and the choice persists with the
workspace. Source hard links remain unsupported, including links between source
and derived paths.

Qualification
-------------

``benchmarks/cargo_workspace.py`` imports an already built Cargo checkout, forks
a writer and a sibling, builds the writer offline, and runs a supplied smoke
command. An optional replacement changes one source file in the writer and
repeats both checks. An independent streaming SHA-256 oracle checks the original
source and target files and the untouched sibling after each build. It checks
file modification times as well as bytes, modes, and symlink destinations.
Git's ignore rules determine ephemeral paths independently of Cowtree's scanner;
ignored secrets are checked for source preservation and excluded from clones.

For example, with a prepared Ruff checkout and its existing dependency cache::

    PYTHONPATH=src uv run --group test python benchmarks/cargo_workspace.py \
      --root /cow-volume/ruff-trial-1 \
      --source /cow-volume/ruff \
      --binary "$PWD/target/debug/cowtree-metadata" \
      --cargo /path/to/installed/toolchain/bin/cargo \
      --cargo-home /path/to/cargo-cache \
      --crate ruff --executable ruff --derived target \
      --derived-hardlinks clone \
      --smoke-command '["{target}/debug/ruff", "--version"]' \
      --expect-output 'ruff 0.16.8'

Use the actual version and package name in the prepared checkout. The root must
be absent and outside the source checkout. The executable must be a previously
installed Cargo toolchain. Cargo's registry and Git dependency caches are copied
to a private Cargo home; credentials and user configuration are not copied.
Private registries that need that configuration require their own prepared
fixture. The harness sets private target and build directories and disables
inherited compiler wrappers, including wrappers set in Cargo configuration.
The default lane measures an existing target across Cargo-home relocation, which
moves registry source paths and can invalidate artifact fingerprints. A second
lane uses ``--prime-owned-source`` on a disposable fixture only. This explicitly
builds its source target using the same private Cargo home that its forks will use,
then freezes the source/cache oracle. Priming is separately timed and may update
the owned target; source code must remain unchanged. This lane measures warm-fork
reuse with stable registry paths. Report the two scenarios separately.
Additional build variables use
``--environment '{"CARGO_INCREMENTAL":"0"}'``. Matching the seed build's profile
and toolchain matters when interpreting freshness.

To exercise an edit, supply ``--edit-path``, ``--edit-content`` (a replacement
file), and ``--edit-expect-output`` together. Smoke arguments are a JSON array,
never a shell command. Build scripts and smoke commands run with the caller's
permissions; use trusted fixtures and commands that respect the private paths.

``report.json`` separates cache-copy, import, fork, and build timings and records
Cargo's actual fresh and compiled artifact counts. ``initial.jsonl`` and
``edited.jsonl`` retain compiler messages; corresponding ``.log`` files retain
diagnostics. Original and eligible clone manifests plus ``setup.json`` preserve
the oracle and setup measurements even when a later build fails. Failed runs keep
their scratch trees for inspection. Run three
fresh roots and report medians for a performance comparison. These timings are
qualification evidence, not a matched comparison against ordinary worktrees or
a proof that all Rust build caches are portable.

The mounted regression test uses two local crates, a path dependency, a build
script with ``rerun-if-env-changed``, a committed lockfile, a real hard-linked
Cargo executable, and explicit source and build-environment changes. It needs
no downloaded crates::

    PYTHONPATH=src uv run --group test pytest mounted/test_cargo_workspace.py -q

Run it on APFS and separately on native-reflink Btrfs or XFS. Process-crash tests
and these build checks do not establish power-loss durability.
