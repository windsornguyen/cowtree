Checked publication
====================

Capture source with ``Workspace::capture``, prepare it with ``Workspace::prepare``,
validate it with ``Workspace::check``, and commit the returned exact candidate.
Preparation against another tip invalidates previous validation. Commit refuses
a missing validation, a different candidate, or modified validated source.

Validation runs an explicit argv command in a temporary warm Git worktree.
Its source must remain identical to the prepared manifest. Successful cache
output is sealed into that tip lineage. The published node reuses the exact
validated Git source identity. Checks are supervised with a deadline and log
threshold. Commands must not daemonize or modify owned workspace metadata.

The authority atomically records publication and its receipt. Local recovery
projects that same result into Git and per-path origins. It preserves working
edits made after capture. ``result`` reads a retained receipt, and retrying the
same candidate returns the same result. A failed check leaves the captured
request available for explicit retry or cancellation.

Run ``cargo test -p cowtree-cli --all-features --test managed``. This checks actual bytes, cache inheritance, validation refusal, exact
retries, and lost replies. It is not a power-loss or distributed-consensus proof.
