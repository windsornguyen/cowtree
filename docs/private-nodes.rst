Private snapshot nodes
======================

``Nodes::seal`` captures source and declared caches into an owned CoW image.
It flushes the image and node record before creating its private Git reference.
The node carries source identities, retained per-path origins, its parent node,
and a source-only Git commit. A fresh node identity distinguishes repeated
content in different history positions. Sealing does not publish to SQLite.

Capture reads the live checkout's ignore rules. Ignored paths are ephemeral
unless explicitly selected as derived caches. A selected cache can sit inside
an ignored directory without inheriting its ignored siblings. Tracked paths,
including dependency lockfiles, cannot be reclassified as derived or ephemeral.
Each node stores its effective policy so later verification uses the same
selection. The optional initial Git parent preserves the source checkout's
history without conflating Git parents with private filesystem node identity.

The owner keeps writers quiescent during capture and excludes mutation of the
snapshot directory afterward. ``verify`` checks source bytes before reuse.
Derived payload is inherited within this lineage and is not part of the Git
source tree. A durable node whose ref update was interrupted can complete that
same ref later. Unfinished capture directories remain unreferenced.

The workspace namespace uses UTF-8 and NFC names. A decomposed spelling is
accepted only when the host resolves it to the same filesystem entry. Names
that differ only by case or canonical Unicode spelling cannot establish
independent authority, including directory prefixes. This portable subset also
applies on case-sensitive mounts. Existing Git-worktree commands retain their
broader filename contract.

Run ``COWTREE_EXPECT_SUPPORTED=1 cargo test -p cowtree-cli --test managed private_checkpoints``. The tests
exercise cache inheritance, private ancestry, source-only projection, alias
rejection, and detection of modified snapshot source bytes.
