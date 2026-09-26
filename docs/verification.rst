Verification
============

The repository has executable metadata and mounted filesystem tests, bounded
TLA+ checks, and three model traces replayed through the runtime. These are
finite conformance checks. They do not establish unbounded safety, liveness,
or refinement for every execution.

Model ownership
---------------

``Core.tla`` is the primary abstract safety model: values, fencing tokens,
origins and dirty keys. Its structural invariant includes exclusive current
authority and bounds on held tokens. ``Fenced`` alone does not imply preservation.

``EpochLog.tla`` adds immutable epochs, frozen proposals, batches, expiry,
rejection and edits after submission. Installation and commit acknowledgement
remain atomic. Its corrected Sync preserves submitted paths; Discard restores
the submitted value when discarding a newer edit.

``Workspace.tla`` retains reservation/activation interleavings and lifecycle
witnesses. ``FencedPublish.tla`` retains captured-token checks, disjoint batches,
finite one-step induction, and controls that remove fencing or replace the
whole tip from a stale leaf. ``CowTree.tla`` is the historical model with finite
invariant-seeded receipts and no integer fencing tokens.

``PublicationRecovery.tla`` adds the process-failure ordering of candidate
construction, durable objects, recorded validation, the tip transaction,
acknowledgement and collection. It assumes the atomic metadata and object
operations supplied by the runtime; it does not implement another service.

These models have different abstractions. No checked refinement connects them.
Core owns basic authority, EpochLog owns proposal lifecycle, Workspace owns
activation races, FencedPublish owns batch admission, and PublicationRecovery
owns the persistence and recovery boundaries described below.

Checker
-------

With Java 11+ and uv, run::

    cargo run -p xtask -- specs --cache /absolute/path/outside-checkout

The runner checks 24 configurations with TLC release 1.8.0, one worker,
a 1 GiB heap, fingerprint polynomial zero and a per-case timeout. It retains
copied inputs, hashes, Java identity, command, coverage, exit status, complete
logs and native JSON counterexamples. Missing expected violations, parse errors,
timeouts and wrong exit codes fail the run. Witness searches intentionally stop
at the exact named invariant violation and are not complete safety explorations.

The vendored upstream checker in ``tools/tla2tools-1.8.0.jar`` has SHA-256::

    066cd246d87a388dfde0f04c3b506007f4c0cb4708a5b5396f0552a005eb75b5

The runner verifies the vendored file before populating or using its cache,
then verifies the cached executable too. It never downloads a substitute.
The JAR is unchanged from the 24-case qualification and retained trace exports.
Its embedded source revision is ``078405c22df8037571860457b2d078e8281bd851``.

Upstream's ``v1.8.0`` tag tracks master builds. The tested asset 569238611
(4,492,834 bytes) was deleted and replaced by asset 569359548 (4,492,966 bytes,
SHA-256 ``9d36716f...``) before hosted CI ran. Both the download URL and asset
API returned the replacement; the original checksum check correctly failed.
Vendoring the previously verified build makes checker retrieval reproducible
offline. See `tool provenance and licenses <../tools/README.md>`_.

The earlier FencedPublish receipt used ``20322939...``; historical Core/EpochLog
receipts used release 1.7.4. Those results retain their original identities.

Replay
------

Run the native replay on a qualifying copy-on-write filesystem::

    COWTREE_EXPECT_SUPPORTED=1 cargo test -p cowtree-cli --test replay

``tests/traces`` retains unmodified JSON exports and manifests binding the
checker, model, configuration and export hashes. Unknown schema/actions or
changed input bytes fail before workspace creation. The native adapter in ``crates/cli/tests/replay.rs`` checks these records.
Each manifest contains a reviewed API action sequence. Expected states come from TLC's export.

The stale-base witness runs two real leaves with disjoint leases. Both capture
proposals before either publishes; the second then publishes over the newer tip.
After every action, replay compares file bytes, installed origins, private dirty
paths, pending values, captured tokens/origins, live lease owners and every
retained snapshot. Commit checks the durable result and an idempotent retry.

Two deterministic wrappers restrict EpochLog.Next to explicit paths through
write, propose, sync, later write, and either commit or expiry/discard. The final
``ReplayIncomplete`` violation exports each completed path. All model invariants
are checked along it. These are selected histories, not random exploration or
proof that the runtime implements all EpochLog actions.

Mapping boundaries
------------------

Runtime versions start with an empty metadata snapshot, then workspace import;
EpochLog starts with the imported content. Runtime tokens use a global counter;
EpochLog increments per path. Replay checks ownership and captured runtime token
identity rather than numerical equality between these counters.

EpochLog.Commit reconciles unrelated clean paths atomically. Replay maps it to
prepare, exact-candidate validation, commit, result, retry and explicit sync.
Checks may allocate private validation leaves; those are outside the projected
model state. Intermediate steps in this compound mapping are not verified by
EpochLog. The mounted recovery tests exercise them separately.

Working file edits remain private until capture. Warm ``Workspace::discard`` restores
the pending value, or the last installed origin without a pending change, through
the installation journal. It does not call metadata ``discard``, whose separate
contract records a revert to the metadata origin as newer local intent.
The selected discard trace has a pending proposal and therefore matches
EpochLog.Discard. General no-pending discard differs: EpochLog reads the newest
tip, while the warm API restores the installed origin.

Failure boundaries
------------------

The existing FencedPublish properties map to these runtime obligations:

.. list-table::
   :header-rows: 1

   * - Model construct
     - Covered obligation
     - Separate runtime boundary
   * - ``CanCommit`` / ``PublishedWithAuthority``
     - Current owner, captured token and matching path origin
     - Recheck inside the final metadata transaction
   * - ``Candidate`` / ``UnmodifiedPathsPreserved``
     - Overlay only the selected disjoint deltas on the current tip
     - Persist candidate bytes and bind validation to their identity
   * - ``LogAppendOnly``
     - Atomic append preserves every earlier snapshot
     - Durable SQL commit, restart and retained-object recovery
   * - ``Grant`` / ``Revoke``
     - Old captured authority cannot become current after regrant
     - Delayed replies and filesystem installation recovery

``Candidate`` is a pure operator; there is no persisted-candidate variable.
There are no separate validate, compare-and-swap, acknowledge, crash, restart
or reclamation actions. In particular, ``AckedRecoverable``, ``NoDanglingRef``
and ``ValidateBoundToId`` are not properties of this model.

FencedPublish.Commit assumes candidate construction, durable object storage,
validation and acknowledgement implement its atomic step. It models none of
those intermediate failures. Runtime tests must separately check interrupted
installation, lost replies, candidate identity, durable receipts and restart.
Neither successful TLC exploration nor trace replay proves fsync ordering,
reclamation safety, namespace behavior, build-cache validity or space savings.

Publication recovery model
--------------------------

``PublicationRecovery`` separates these boundaries:

.. list-table::
   :header-rows: 1

   * - Action
     - Runtime meaning
   * - ``Prepare``
     - Record the immutable candidate identity, parent and captured token; pin its inputs
   * - ``Build``
     - Construct candidate bytes in volatile process state
   * - ``Persist`` / ``MarkReady``
     - Flush immutable object bytes before recording that the candidate is ready
   * - ``Validate``
     - Finish a successful exact-candidate check and persist its identity/parent binding
   * - ``Commit``
     - Compare the parent to the current tip, recheck token and validation, then atomically commit tip and receipt
   * - ``Acknowledge``
     - Return a durable publication result, including retry after a lost reply
   * - ``Crash`` / ``Restart``
     - Lose volatile construction state; reopen durable metadata and client records
   * - ``Pin`` / ``Unpin`` / ``GC``
     - Preserve client roots, preparation inputs and retained publications during reclamation

``AckedRecoverable`` requires acknowledged roots to remain published and durable.
``NoDanglingRef`` requires every retained publication, ready pending candidate,
preparation parent and client pin to name a durable object. ``ValidateBoundToId``
binds each committed request to its own validation identity and parent.
``PublishedWithAuthority`` checks captured tokens at commit; ``LogAppendOnly``
preserves prior committed history. Revocation can interleave with every
preparation, validation and publication phase.

The bounded instance has two independent writer keys, two immutable candidate
IDs, and tokens through two. Each key has at most one pending request. Every
publication remains within the retained window in this instance; acknowledgement
does not promise indefinite retention after the runtime's documented expiry.
No symmetry, state constraints, depth limit or fairness restrict the exploration.

The safety run exhausts its queue: 1,252,175 generated states, 324,788 distinct
states, depth 27. Two deliberate controls require native JSON counterexamples:
``PublicationWrongValidation`` permits a different candidate's validation and
violates ``ValidateBoundToId``; ``PublicationPrematureGC`` drops acknowledged
publication roots from collection protection and violates ``AckedRecoverable``.
The runner requires the named invariant and exit 12 for each control.

Objects represent complete immutable candidate closures. Hash collisions,
content deduplication, partial file writes, filesystem namespace interpretation,
validation command behavior and the mechanics of reconstructing client pins are
outside this abstraction. ``Validate`` assumes source identity has been checked;
a crash before its durable record is modeled as not completing that action.
SQLite commits and acknowledged object flushes are assumed durable and atomic.
Keys stand for disjoint admitted proposals; the separate FencedPublish model
checks that admission obligation.

This is bounded process-crash qualification. It does not simulate power loss,
hardware faults, broken fsync guarantees, or prove refinement to the runtime.
The three mounted EpochLog traces do not replay this additional failure model;
the runtime's separate interruption tests provide implementation evidence.
Git ref projection, per-file installation and their journals are not separate
actions in this model and retain that implementation-test boundary.

Proof track
-----------

Core's finite invariant-seeded configurations constrain tokens and initial
history ghosts. They are not unbounded induction proofs; larger configurations
remain unqualified unless their queues finish. No fairness/liveness is claimed.

Plain Lean or TLAPS may establish Init => Inv and Inv /\ Next => Inv' with all
structural hypotheses. Veil is an optional separate accelerator experiment.
No admitted proof, Lean/Veil/TLAPS theorem, Verus, Kani or Loom result is claimed.
