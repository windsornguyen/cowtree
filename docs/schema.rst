Declarative SQLite schema
=========================

``crates/metadata/schema.sql`` is the single editable database declaration.
Atlas Community generates ``src/schema.sql`` from it. The generated file has a
watermark, the pinned generator revision, and the declaration's SHA-256 checksum.
Edit the declaration and regenerate; do not edit the generated SQL.

Prerelease storage accepts only the current declaration. There is no numbered
schema ledger, historical migration directory, or automatic upgrade path.
Opening a store compares its actual user tables, indexes, views, and triggers
against the included generated schema and refuses a mismatch. Cowtree does not
migrate the schema or change logical row values. SQLite may still recover or
checkpoint already committed WAL pages when opening or closing the connection,
so rejection does not promise byte-identical database and journal files.
The ownership application ID remains. Publication epochs and request
sequences still identify history and retries; they are not schema versions.
The workspace record likewise has no schema-version field or implicit layout upgrade.

Generator
---------

The public-source `Atlas Community Edition`_ is Apache-2.0 licensed. The CLI is a
development and CI dependency; Cowtree's installed runtime continues to use
rusqlite with its bundled SQLite. PostgreSQL-specific tools such as
pg-schema-diff cannot generate Cowtree's SQLite plans.

Install Go 1.26.5 and build the pinned public source::

    git clone https://github.com/ariga/atlas.git .tools/atlas-source
    git -C .tools/atlas-source checkout --detach 9a6bc601212130aaaefcbc8dd36c710baf9716ff
    GOTOOLCHAIN=local go -C .tools/atlas-source/cmd/atlas build \
        -mod=readonly -trimpath -o ../../../atlas .

``tools/atlas-revision.txt`` pins the source. The wrapper checks Go build metadata
for that exact unmodified revision. It does not use the proprietary distribution,
an Atlas account, or cloud services. Git and Go must be available when generating
schema artifacts. Tool sources and binaries remain in ignored ``.tools/``.

Run from the Cowtree checkout::

    cargo run -p xtask -- schema generate
    cargo run -p xtask -- schema check

``generate`` writes the current initialization SQL. ``check`` fails if regeneration
differs from the checked-in output. CI builds the pinned Community source and
runs both this check and the actual SQLite constraint/data-preservation tests.
Commit the declaration, generated SQL, and relevant tests together.

Generate a review diff
----------------------

Save the declaration before editing, then generate a plan against the new source::

    cp crates/metadata/schema.sql /tmp/cowtree-before.sql
    # Edit crates/metadata/schema.sql.
    cargo run -p xtask -- schema diff --from-schema /tmp/cowtree-before.sql \
        > /tmp/cowtree-schema-diff.sql

The wrapper freezes both input files into owned temporary files. It prints SQL
with a watermark and both declaration hashes. It does not open an existing
workspace database or apply the plan. Retain review diffs outside the repository;
there is no migration numbering or history to maintain during prerelease.
The baseline is the earlier declaration compiled by the same pinned generator,
as used by Cowtree initialization. It is not an arbitrary live database: generated
index names and physical DDL can differ from executing the source declaration directly.

Community Edition silently omits SQL views and triggers. The wrapper uses SQLite
inspection to reject those declarations before generation. Cowtree's ``views``
table is an ordinary table and is supported. Generator tests exercise real
primary, unique, check, and foreign-key constraints, defaults, and row preservation.
Declarations contain ordinary tables and indexes. SQLite authorization rejects
user-data writes, attached databases, virtual tables, and PRAGMA settings, which
are not part of the generated schema contract.
Name referenced foreign-key columns explicitly; this Community revision rejects
implicit primary-key references. Tool errors retain their diagnostic message.
The wrapper also rejects ``COLLATE``, ``DEFERRABLE``, and ``ON CONFLICT`` clauses:
the pinned differ can omit them and change uniqueness or transaction behavior.
The guard excludes quoted literals, identifiers, and comments. Qualify additional
SQL features before expanding this supported declaration subset.

Some SQLite changes require rebuilding a table. Atlas's generated rebuild plan
can toggle foreign-key enforcement and does not itself provide an enclosing
transaction. Treat this output as a review artifact, not a crash-qualified live
upgrade command. Automatic application requires a separate tested contract.
Create a new prerelease store when its declaration changes; keep an old store
with its original Cowtree bundle if it contains work that must be recovered.

.. _Atlas Community Edition: https://atlasgo.io/community-edition
