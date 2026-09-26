Contributing
============

New external contributors must open a
`Contribution interest issue <https://github.com/windsornguyen/cowtree/issues/new?template=contribution.yml>`_
before opening a pull request. Describe the proposed work and wait for a
maintainer to approve its scope and vouch for you.

The repository owner and collaborators with write access pass the Vouch gate
automatically. Contributors already vouched in `VOUCHED.td <VOUCHED.td>`_ do not
need another contribution-interest issue for each PR. Discuss substantial
changes with a maintainer before implementing them.

Bug reports and questions are welcome without a vouch. Use a
`blank issue <https://github.com/windsornguyen/cowtree/issues/new>`_ for those.

Contributor access
------------------

1. Open a contribution-interest issue with the problem, proposed change,
   intended scope, and validation plan. Link any existing issue or discussion.
2. Discuss the scope with a maintainer. A maintainer grants access by commenting
   ``vouch`` on your issue, or ``vouch @your-handle`` to name you explicitly.
3. Wait for the ``Vouch: Manage`` workflow to record your handle in
   ``VOUCHED.td`` on the default branch. A comment alone does not complete this
   step. Do not add yourself to the file in your PR.
4. Open a focused PR that links the approved issue and includes test results.
   State the test environment and any skipped or unavailable checks.

A vouch grants contribution access. It does not approve a change or waive code
review and required checks. PRs from contributors without access are closed by
the Vouch workflow.

Maintainers
~~~~~~~~~~~

Repository collaborators with the ``admin``, ``maintain``, or ``write`` role can
manage access through issue comments. Use ``vouch`` to grant access,
``unvouch`` to remove a listing, or ``denounce`` to record a blocked contributor.
Each command targets the issue author by default; add ``@handle`` to target
someone else. Review the issue before granting access.

Confirm that ``Vouch: Manage`` succeeds and updates ``VOUCHED.td`` on the default
branch. These workflows must be installed on the default branch before issue
comments or PR events can use them.

For a PR whose branch is in this repository, open **Actions**, select
**Vouch: Check PR**, and use **Run workflow**. Select the PR's current head
branch and enter its ``pr-number``. The check rejects a different branch or
commit. Maintainers can also dispatch that registered workflow with the CLI::

    gh workflow run vouch-check.yml --ref PR_HEAD_BRANCH -f pr-number=PR_NUMBER

For a fork PR, reopen it after the vouch is recorded, or push an update to an
open PR. These events run the trusted default-branch gate. If a check reports
an authentication or workflow error, repair the error and rerun the check.
An owner does not need to request a vouch to repair workflow authentication.

Development rules
-----------------

Use small commits with conventional commit messages. Use Cargo for dependencies and native checks. Do not use ``pip``, ``poetry``, or ``requirements.txt`` in this repo.
There is no CLA.

Setup
-----

Install Rust 1.85 or later, a C toolchain, and Git. Build the runtime::

    cargo build --locked --release -p cowtree-cli

Checks
------

::

    cargo fmt --all --check
    cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
    cargo test --locked --workspace --all-targets --all-features
    cargo doc --locked --workspace --no-deps

Native filesystem tests require APFS, reflink-capable XFS or btrfs, or a supported
Windows volume. Set ``COWTREE_EXPECT_SUPPORTED=1`` to make missing clone support
fail the run. ``scripts/fs_matrix.sh`` exercises Linux support and refusal cases
on owned loopback images. The Windows script uses its own temporary virtual disk.

Dependencies and benchmarks
---------------------------

Use ``cargo add`` and ``cargo remove`` and commit ``Cargo.lock``. Keep each crate
focused on its ownership boundary. ``lib.rs`` and ``mod.rs`` are documented indexes.

Measure complete creation with the same fixture and alternating order::

    cargo run --release -p cowtree-cli --example benchmark -- \
        --binary target/release/cowtree --files 8192 --trials 5 \
        --output /tmp/cowtree-benchmark.json

Keep generated measurements, logs, and plots outside the checkout. Commit runnable
methods and reviewed analysis. Release evidence needs source identities, exact
commands, and retained receipts. See `production gates <docs/production-readiness.rst>`_.

The schema and protocol model checks also run through Cargo::

    cargo run -p xtask -- schema check --atlas .tools/atlas
    cargo run -p xtask -- specs --cache /absolute/path/outside-checkout

Atlas must match ``tools/atlas-revision.txt``. The model checker is vendored and
checksum-verified. These checks never modify a workspace authority.

Scope
-----

Cowtree owns CoW worktrees, private snapshots, checked publication, and recovery.
Agent orchestration, dashboards, and branch policy belong in callers. Git remains
the repository authority and is invoked through explicit argument vectors.

Generated GitHub Actions
------------------------

Workflow sources live in ``ci/`` and use Hollywood Actions 0.0.5. Do not edit
``.github/workflows/`` or ``.github/actions/ci/`` by hand. With Node 24.11 or newer::

    npm ci --ignore-scripts
    npm run ci generate
    npm run ci check

Commit the TypeScript sources and generated outputs together. See
`ci/README.md <ci/README.md>`_ for the generation check and Vouch trust boundary.
