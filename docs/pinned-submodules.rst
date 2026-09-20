Pinned submodule materialization
================================

Managed workspaces reject submodules by default. To opt into read-only
materialization of clean, initialized direct dependencies::

    cowtree workspace --root ../store init --source . \
        --binary ./target/debug/cowtree-metadata --submodules materialize-pinned

Python uses ``PathPolicy(submodules=SubmodulePolicy.MATERIALIZE_PINNED)``.
The policy enum lives in ``cowtree.submodule_types``.

This mode places dependency files in Cowtree's private Git projection as regular
source entries. It does not preserve nested Git repositories. Each dependency's
``.git`` control state is excluded, and a generated ``.cowtree-pin`` records its
original path and commit. The source repository's gitlinks, ordinary refs, and
staged entries remain unchanged. Receipts and dependency bytes enter snapshot identity.

The Rust authority rejects publication grants for dependency namespaces and
their ancestors. Private file edits remain isolated, but capture refuses to
publish or retain a checkpoint containing changed dependency source. Ordinary
parent-source edits still follow the checked publication workflow.

Admission rejects dirty, missing, mismatched, nested, untracked, or ignored child
contents. Escaping, absolute, or cyclic dependency links are unsupported.
Dependencies cannot overlap cache or ephemeral policy. Fresh temporary Git
indexes verify both input and copied bytes without trusting stat-cache hints.
No fetch, submodule update, or implicit dependency version change occurs.

Validate the complete flow and refusal cases on a native CoW filesystem::

    cargo build -p cowtree-metadata
    uv run pytest -q mounted/test_submodules.py
