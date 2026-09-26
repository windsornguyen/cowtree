Frozen source requests
=======================

``Workspace::capture`` seals the current private source, acquires matching
authority, and records one request identity before submitting its values.
The authority's returned proposal must match those captured values. Later
working edits do not change the request. A leaf admits one pending publication.

An interrupted submission reuses its immutable node and request sequence.
``abort`` records cancellation intent before retiring that identity, releases
unused upload pins, and preserves working files. The authority can retire the
next identity even when submission never completed. It never reuses that
sequence after collection. A committed publication cannot be aborted.

The caller keeps writers quiescent during capture. Failed or stale authority
remains an explicit failure with the capture retained. Use cancellation or
explicit resolution instead of changing the request's inputs.

Run ``cargo test --locked -p cowtree-metadata --test cancellation`` and
``cargo test -p cowtree-cli --all-features lost_capture``.
