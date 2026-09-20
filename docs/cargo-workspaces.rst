Cargo workspaces
================

Cowtree owns private filesystem views. BSMR remains the action-cache authority:
it decides which inputs identify a build and which cached outputs are reusable.
An inherited Cargo target directory is disposable derived state. Its presence
does not establish a cache hit or make an artifact portable to another compiler,
platform, environment, or absolute checkout or registry-source path.

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
