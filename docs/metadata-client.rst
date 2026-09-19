Python metadata client
======================

``cowtree.metadata.Metadata`` owns one local Rust process and exchanges one
JSON request and response at a time. Pydantic validates response records.
The Rust authority remains the only writer of its SQLite schema and object
directory. A domain failure raises ``CowtreeError`` and does not become an
empty result. ``MetadataError.reason`` preserves the authority's failure code,
so callers distinguish a changed tip from I/O failure without parsing messages.
A subsequent valid request can use the same process.

Select the executable explicitly. Missing executables fail without attempting
an installation or selecting another implementation. The client enforces the
128 MiB protocol line limit and a response deadline, including partial lines.
Closing the context closes input, waits for exit, and reaps an unresponsive
process. This is an internal adapter for the filesystem workspace lifecycle.

Build and verify the real boundary with::

    cargo build --locked -p cowtree-metadata
    uv sync --locked --group test
    uv run pytest -q integration

The integration profile requires the compiled executable and fails if it is
missing. The SQLite CI jobs build it and run the profile on Linux and macOS.
The ordinary Python test profile remains independent of Rust installation.
