Versioned workspace protocol
============================

Status: design proposal. The implemented API remains the four worktree
operations in the `README <../README.rst>`_. This document proposes a metadata
coordinator for many editable worktrees, called leaves. It does not add a
metadata service, distributed filesystem, or new CLI commands.

Rounds and snapshots
--------------------

A workspace starts with an immutable snapshot ``S_0``. Its version ``t`` counts
committed epochs, not elapsed time or individual proposals. One accepted batch
advances the version once, even when it contains several proposals.

``S_t`` maps normalized paths to content identifiers and file metadata. A
content identifier names immutable bytes. An absent entry is explicit, so a
delta can express deletion. A snapshot includes file types, executable modes,
symlink text, and namespace structure, not just regular-file contents.

A caller that already holds ``(workspace, t, root_hash)`` can read that snapshot
without discovering the current tip. The coordinator must still provide a
linearizable tip read: it returns a version consistent with completed commits.
Content addressing provides stable identity and verification. Publication must
also make the referenced data durable and available under the read contract.

For example, leaf A reserves ``src/a.py`` at version 4. Leaf B publishes a
change to ``src/b.py``, producing version 5. A may still publish its change
against version 5 if A's reservation remains current and its recorded origin
for ``src/a.py`` still matches that path in ``S_5``. Its delta is applied to
``S_5``. Replacing the tip with A's whole version-4 tree would lose B's change.

Ownership
---------

.. list-table:: Authoritative state
   :header-rows: 1
   :widths: 28 27 45

   * - State
     - Owner
     - Contract
   * - Snapshot objects
     - Content store
     - Immutable after their content identifiers are assigned.
   * - Tip, lease generations, commit records
     - One workspace coordinator
     - Changed through one ordered transaction boundary.
   * - Path origins and editable views
     - Leaf client
     - Each origin records installed committed content. Views hold local edits.
   * - Proposal bytes and identity
     - Submitting leaf
     - Frozen until the coordinator's result is reconciled.
   * - Resolution records
     - Resolution service or explicit reviewer
     - Record exact inputs, policy identity, and approved output.

A lease grants write authority over a resource set. Each grant carries a
monotonically increasing integer generation, a fencing token. The coordinator
compares tokens with its current state whenever authority is used. Tokens do
not wrap, reset after restart, or get reused for a recreated leaf identity.

An old holder can keep private edited bytes after losing authority. It need
not receive an instantaneous revocation notification. Its old token prevents
those bytes from being accepted as an ordinary publish or release.

Proposed operations
-------------------

These names define responsibilities, not an RPC encoding or released Python API.

.. list-table:: Metadata interface
   :header-rows: 1
   :widths: 25 75

   * - Operation
     - Effect
   * - ``snapshot.read(version)``
     - Read a retained immutable snapshot. Report unavailable versions explicitly.
   * - ``tip.read()``
     - Read the current committed version and root through the coordinator.
   * - ``leaf.create(version)``
     - Create an active leaf from a retained snapshot with no write authority.
   * - ``lease.reserve(paths)``
     - Atomically reserve all requested resources, issue current tokens, and pin
       their origins from the current tip. Refuse conflicting reservations.
   * - ``lease.activate(tokens)``
     - After installation, verify the reservations are still current before
       permitting edits through them.
   * - ``lease.release(tokens)``
     - Release only reservations still owned by the caller under those tokens.
       Retain unpublished edits with stale authority.
   * - ``proposal.submit(id, origins, delta, tokens)``
     - Register immutable proposal inputs. Reuse the same identity and body when
       retrying an uncertain result.
   * - ``proposal.result(id)``
     - Read the recorded outcome of a submitted proposal without creating a new
       publication.
   * - ``publish.batch(ids, expected_tip)``
     - Validate the selected proposals and atomically publish one new epoch.
       Record a result for every accepted proposal.
   * - ``resolution.prepare(inputs, policy)``
     - Produce or approve a candidate merge with its complete input identity.
   * - ``leaf.drop()``
     - Mark the leaf inactive and invalidate its reservations. Drop does not
       publish outstanding edits.
   * - ``leaf.discard(paths)``
     - Explicitly abandon retained local edits before resynchronizing those paths.

Reserve and activate are separate transitions. Installing files can take time.
A reserved path is not editable until installation finishes and activation
confirms its token. Expiry or revocation between those transitions makes
activation fail. Installation must not silently overwrite retained dirty data.
The caller must preserve it for resolution or explicitly discard it first.
Use generation-specific staging, or serialize installation attempts for each
leaf and path. A delayed installation from an old generation must not overwrite
a newer activated view, even when both grants name the same leaf.

A coordinator can use deadlines to decide when to revoke an idle reservation.
Revocation advances its authoritative generation. Safety follows from checking
that generation, not from comparing clocks on the leaf. Deadlines and fair
scheduling determine progress and are separate liveness requirements.

Namespace resources
-------------------

Compare normalized path components, not raw string prefixes: ``a`` overlaps
``a/b`` but not ``ab``. Define normalization, case sensitivity, and Unicode
handling for the workspace before granting reservations. Resolve symlinks only
under an explicit policy. A pathname reservation must not accidentally become
a reservation on whatever a changing symlink currently points to.

Directory removal or replacement conflicts with changes to descendants.
Creation reserves the relevant directory entry. Rename reserves the source,
destination, affected descendants, and namespace entries needed to preserve
structure. Its deletion and insertion publish in the same proposal. Snapshot
validation must reject invalid file/directory combinations even when regular
file write sets appear disjoint.

Publication boundary
--------------------

The leaf freezes writes to a proposal's paths until it reconciles the result.
Other paths can remain editable. Supporting edits after submission requires an
additional per-path edit generation so an acknowledgment cannot clear newer
work. That pipelining is outside this initial proposal.

A coordinator accepts an ordinary proposal only when all these conditions hold:

* The leaf is active and owns current, activated tokens for every changed
  resource. The delta is contained in that reservation set.
* The proposal's recorded origins for changed paths still equal those paths at
  the tip being extended. An older workspace version alone is not a conflict.
* Every referenced content object is durable and available, and the resulting
  snapshot satisfies namespace and metadata rules.
* Accepted proposals have pairwise-disjoint resource sets within this batch.
  One holder submitting twice does not bypass this rule.

Resolve proposal identities before admission. Reusing an identity with different
inputs is an error, including before it commits. An already committed identity
with matching inputs returns its recorded result without advancing the tip or
requiring its former leases to remain current. Only uncommitted proposals go
through admission again.

For admitted deltas ``D_1 ... D_k`` with disjoint domains, snapshot composition is::

    S_(t+1) = S_t overlay D_1 overlay ... overlay D_k

The order of those disjoint overlays does not change file values. Admission,
lease validation, tip comparison, and commit-result recording nevertheless
form one atomic coordinator transaction. Every successful ordinary publication
has a linearization point at that transaction. Checking a token and then doing
an independent compare-and-swap (CAS) on the tip is insufficient: revocation
can occur between the two operations.

Upload and candidate construction can run in parallel before this boundary.
If the tip or reservation state changes before commit, admission must be
rechecked against the new state. The success response is sent after the
publication's durability requirements hold. After a lost response, the caller
queries the same proposal identity instead of creating a second mutation.

Reservations can span many epochs. A batch boundary does not expire them or
force a leaf to abandon work on unchanged paths. A producer can submit a new
batch as soon as it has a valid candidate and authoritative commit transaction.
There is no wall-clock tick that every leaf must join.

Review invariants
-----------------

The proposal requires these properties. The bounded model covers the subset
identified in the `model README <../specs/README.md>`_.

1. **Immutable history.** Published snapshots never change. The tip identifies
   the last committed epoch, and every successor extends that history.
2. **Exclusive resources.** Concurrent current reservations do not overlap
   between holders, including namespace ranges.
3. **Fenced authority.** Edits through the protocol, activation, ordinary
   publication, and release require the current holder and token.
4. **Installed origins.** An active, activated current holder's origin for a
   reserved path equals that path in the current tip when no publication for
   that path awaits reconciliation. Its dirty view may differ. An unresolved
   publication keeps that path frozen until its recorded result is reconciled.
5. **Clean active views.** An active leaf's non-dirty view equals its origin.
   Dropped leaves have no such local-view obligation.
6. **Local dirty classification.** A dirty path has current authority or retains
   a superseded token. Stale private bytes have no ordinary publish authority.
   One expiry must not disable checks for every leaf through a global flag.
7. **Atomic admission.** Each committed proposal has current authority and
   matching path origins at the same boundary that advances the tip.
8. **Complete publication.** All objects referenced by a visible tip meet the
   store's durability and availability contract. Namespace metadata is valid.
9. **Stable retry identity.** One proposal identity names one input body and at
   most one recorded result. A retry cannot silently become another publish.

An induction argument must show initial validity and preservation by every
transition, including reservation, activation, edit, release, revoke, publish,
resolution, and drop. Checking reachable states under TLC is useful evidence
for a finite model. Checking ``Inv /\\ Next => Inv'`` from all finite states
satisfying ``Inv`` is a different obligation. Neither establishes an unbounded
theorem merely by displaying a large state count.

The intended preservation argument, conditional on the contracts above, is:

* Initial leaves have a valid immutable origin, matching clean views, and no
  reservations or pending proposals.
* Reservation and activation establish current origins before edits are allowed.
  Editing changes a private view and its dirty set, not the immutable snapshot.
* Publication changes only admitted resources. Other current holders' origins
  remain valid because those resources are disjoint. Publishing holders adopt
  the committed origins during reconciliation while their paths remain frozen.
* Revocation and release advance generations. Retained edits become stale and
  lose ordinary publication authority. Drop removes only that leaf's active
  obligations and cannot weaken checks for another leaf.
* A contested resolution re-enters the same admission rules. Its merge driver
  must separately establish the validity of its output and complete input set.

The finite model also abstracts publication and client reconciliation as one
transition. It does not check lost acknowledgments, in-flight token-bearing
messages, or overlapping installation attempts. Those boundaries need separate
implementation tests and a more detailed model before deployment.

The supplied summary reports successful checks of earlier ``CowTree.tla`` and
``EpochLog.tla`` models, but their source and configurations were not supplied.
Those counts are not reproduced or certified here. The checked-in model is a
new, bounded review model with its own exact scope and results.

Contested changes and resolution reuse
--------------------------------------

A stale proposal cannot use a merge result to bypass fencing. Preserve its
original base and edited bytes, obtain current reservations, and compute a
candidate against the current tip. Publishing that result must validate those
reservations and the exact resolution inputs at the ordinary commit boundary.
If the tip changes on an input path, recompute or reapprove the resolution.

For a fixed pure merge driver, three content hashes can identify its content
inputs. A reusable cache key must additionally identify the driver, version,
configuration, relevant path/type/mode context, and every extra input it reads::

    resolution_key = hash(driver_identity, policy, base, ours, theirs,
                          metadata_context, additional_input_hashes)

A lockfile generator can depend on manifests, toolchain, platform, and registry
state. Pin those inputs or treat the output as an explicitly approved result.
A human resolution is a recorded decision, not proof of a deterministic merge
function. Cached output still requires ordinary publication validation.

Isolation and scaling limits
----------------------------

This is a reservation protocol. Synchronizing selected leased paths can leave
a leaf reading a mixture of versions, so the current proposal does not claim
snapshot isolation. That requires a defined transaction-wide read snapshot.
Serializability additionally requires validation or reservations for read
and predicate dependencies. Disjoint writes alone do not preserve arbitrary
application invariants.

For example, two leaves can each observe two enabled services and disable a
different one. Their disjoint writes still violate an application rule that
at least one service remains enabled. That rule needs a shared reservation or
read-dependency validation.

Scale independent workspaces with separate coordinators. Multi-region delegated
prefix ownership requires its own handoff fencing, global ordering, and
acknowledgment protocol. Merging version vectors later does not establish the
single-tip contract. Atomic cross-workspace moves similarly require a separate
transaction and recovery design. Neither extension is proved by this model.

Related work
------------

* `Aria <https://www.vldb.org/pvldb/vol13/p2047-lu.pdf>`_ executes a batch against
  a shared snapshot and selects commits through conflict analysis. Its
  reservations are not a proof for these long-lived leaf leases.
* `Calvin <https://www.cs.yale.edu/homes/thomson/publications/calvin-sigmod12.pdf>`_
  schedules transactions deterministically and acquires locks before execution.
  This proposal does not specify Calvin's input replication or scheduling.
* `FoundationDB conflict ranges
  <https://apple.github.io/foundationdb/developer-guide.html#conflict-ranges>`_
  account for read and write dependencies. Its strict serializability guarantee
  is stronger than exclusive write reservations without read validation.

Implementation review
---------------------

Before implementing the metadata service, settle the resource normalization
rules, durable coordinator transaction, retention policy, proposal retry
records, and resolution policy identity. Map each model transition to its
actual transaction or client-side activation boundary. Replay its witnesses
through the implementation before claiming refinement.

Any backend using persistent epochs or pending logs must separately prove
which component owns a leaf's writable state and how an imported snapshot is
installed without losing uncommitted bytes. This repository contains no such
backend integration. No foreign-epoch installation claim is made here.
