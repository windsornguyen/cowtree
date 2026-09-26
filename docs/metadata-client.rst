Metadata client
===============

Managed workspace sessions call ``cowtree_metadata::Store`` directly. Each session
holds one SQLite connection while it reconciles filesystem journals and authority
operations. The workspace lock is inherited by mutating Git children.

The authority returns typed errors with stable wire codes, retry actions, and
operation details. The native CLI preserves that structured context in error
responses. Object installation verifies content identities and retains origin
objects through authority records and client pins.

``cowtree-metadata`` remains a diagnostic JSON-line executable for exercising the
low-level protocol. It is not required by normal managed commands. Build and test
that interface with::

    cargo build -p cowtree-metadata
    cargo test -p cowtree-metadata --all-features

See ``crates/metadata/README.md`` for the protocol and retention obligations.
