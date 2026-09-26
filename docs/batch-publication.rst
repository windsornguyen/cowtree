Checked batch publication
=========================

``Workspace::prepare_batch`` publishes disjoint captured changes from several managed
leaves in one SQLite epoch. Capture each writer with ``Workspace::capture``, then
prepare the complete membership, check its union once, and commit that exact batch.

.. code-block:: python

   batches = Workspace::prepare_batch
   candidate = batches.prepare(identities=(left.id, right.id))
   validation = batches.validate(
       candidate=candidate,
       command=("python", "-m", "pytest"),
       timeout_seconds=300,
   )
   result = batches.commit(candidate=candidate)

Preparation freezes complete sorted membership against the current tip. Every
member has its own request and attempt, with the same parent and source root.
Overlapping paths fail; the caller must resolve them before preparing the batch.

Validation uses the existing supervised check worktree. It materializes the whole
union from the warm parent, runs the explicit command, rejects source changes,
and seals the exact checked Git identity and eligible cache outputs. Every member
records that same validation node, command, and log with its own candidate identity.
An unchecked member, changed membership, or newly prepared attempt invalidates
the batch. Members cannot publish separately through ``Workspace::commit``.

Commit atomically records all source changes and receipts in SQLite. Every receipt
names the same epoch and root. Existing publication recovery projects the shared
checked node into the warm tip and acknowledges each writer. Later working edits
remain intact. The local epoch record retains the complete receipt list and shared
validation log; future forks inherit the checked union and its eligible caches.

After losing the commit response, repeat ``commit`` with the exact batch candidate.
A workspace session first completes any interrupted per-leaf acknowledgement.
The authority verifies the full candidate before returning an existing receipt.
Individual receipts remain available through ``Workspace::result`` within their
retention window. Preparation interrupted before returning a candidate can be
repeated with the same leaf identities. Aborting or dropping any pending member
invalidates the old batch; prepare the remaining captures again explicitly.

Run ``cargo test -p cowtree-cli --all-features --test managed checks_cannot_publish`` on a native copy-on-write filesystem.
The tests cover union validation, cache inheritance, later edits, candidate identity,
lost replies, and partial local acknowledgement. They model process interruption;
they do not establish a sudden-power-loss guarantee.
