# Workspace protocol models

The current runner uses verified official TLC 1.8.0 and checks 24 cases.
`docs/verification.rst` owns the current pin, replay mapping and failure boundaries.
The recorded sections below retain their historical checker identity.


`Workspace.tla` is a bounded review model for the proposed workspace
protocol. It is an abstraction, not a complete description of the mounted runtime.

Three further modules sit beside it, smallest first. `Core.tla` is the protocol
under all of them: a value and a fencing token per key, `Grant` and `Commit`,
an invariant combining `Fenced` with type, exclusivity, and token bounds
(see `docs/verification.rst`). There is no checked refinement between models. `EpochLog.tla` is the service with its
log made explicit: proposals, batching, holders, expiry, rejection.
`CowTree.tla` is the first formulation, kept for its induction receipts. Their
configs and receipts are in the sections below; `scripts/check_specs.py` runs
the selected bounded cases, alongside FencedPublish.

## State and operations

`epochs` is an immutable sequence of snapshots. Its zero-based tip counts committed
epochs, with one leaf publish per epoch in this model. A snapshot maps paths to symbolic content
values. `owner` and `generation` form an exclusive reservation table; generations
increase on reservation, revocation, release, and drop, and never wrap.

| Action | Boundary represented |
| --- | --- |
| `Reserve` | Claim an unowned path with a new generation and pin its current content. Dirty retained edits must be discarded explicitly first. |
| `Activate` | Check the reservation's current owner and generation, install its pinned content, and permit edits. Reservation and activation can interleave with revocation. |
| `Edit` | Change a path while its installed reservation is current. |
| `Publish` | Check every dirty path's installed owner and generation, then append one snapshot and reconcile the publishing leaf's origins atomically. A different path's intervening publish does not invalidate this proposal's base version. |
| `RejectStale` | Reject retained edits that no longer have current authority, leaving history unchanged. |
| `Revoke` / `Release` | Clear ownership and increase the generation while retaining the old view and edits. |
| `DiscardStale` | Explicitly discard a stale path's retained edits and installation token. |
| `Drop` | Invalidate every owned path and remove the leaf from the active set. Its view is no longer meaningful. |

Each leaf has one mutable proposal: its complete dirty set. The model serializes
operations for each leaf/path and publishes one leaf per version. There is no
proposal queue or batch action. An implementation that batches proposals must
reject overlapping write sets, including proposals from the same holder. Lease
exclusivity alone does not establish that queued proposals are disjoint.

## Checked properties

`Workspace.cfg` selects seven state invariants:

| Property | Claim |
| --- | --- |
| `TypeOK` | State stays within the configured domains. |
| `OwnerActive` | Every reservation belongs to an active leaf. |
| `Exclusive` | At most one leaf has current authority for a path. |
| `CurrentOrigin` | An active, currently authorized installed path has the current snapshot as its origin. |
| `CleanView` | Every clean path of an active leaf equals its origin. Dropped views are excluded. |
| `DirtyScope` | Every active dirty path has an installed token that is either current or demonstrably older than the current generation. Revocation affects that path, not an eternal workspace-wide history flag. |
| `AcceptedAuthority` | The most recent accepted publication checked current authority for its entire dirty set. |

Two temporal safety properties check every transition: `HistoryAppendOnly`
preserves every existing snapshot, and `MonotoneFences` prevents generation
rollback. TLC explores reachable states from `Init`; this is not a check of
`Inv /\ Next => Inv'` from arbitrary invariant-satisfying states, nor a proof for
arbitrary numbers of leaves, paths, tokens, or publishes.

## Bounds and exclusions

The safety configuration has two leaves, two paths, two content values (`0`, `1`),
at most two published versions after version zero, and generations `0..3`.
Generation zero grants no authority. Actions that would exceed these bounds are
disabled. The witness and mutation configurations use the same limits, with one
path for the stale-token and broken-fence checks. There are no additional state
constraints, action constraints, depth limits, symmetry reductions, or fairness
assumptions. `Next` permits explicit stuttering, so deadlock/liveness claims are
outside this check.

Atomic `Publish` combines server commit and local acknowledgement reconciliation.
A real server may commit before the leaf learns the result. Crashes, lost replies,
publication identifiers, retries, and the interval with an unresolved publication
are excluded. In that interval the implementation must not assume the local
origin already equals the committed tip.

Messages in flight are also excluded: actions read the model's current token,
not a captured request token. The model therefore does not check an old install,
release, or publish request arriving after the same leaf has reacquired a path
with a new generation. Real requests need captured fencing tokens, and staged
installations need generation-specific identities and token-checked activation.
Serializing the model's leaf/path operations is not proof of those races.

The model omits directory/prefix leases, rename, content hashing and collisions,
filesystem copying, storage failures, distributed replication, and contested
merge algorithms. Retained stale edits have no ordinary-publish bypass. A future
contested route must preserve the original merge inputs, reacquire and install
current authority, then validate both the captured generation and merge base at
commit. It must not reuse `FenceChecks = FALSE`.

EpochLog now has three hash-bound runtime replay fixtures; see
`docs/verification.rst`. Workspace itself still has no checked refinement.

## Reproduce

Install Java 11 or later and `uv`, then run from the checkout:

```sh
uv run python scripts/check_specs.py --cache ../cowtree-tla-cache
```

Use `--java /absolute/path/to/java` to select a runtime, `--case Workspace` to
select one configuration, or `--timeout-seconds 180` to set each TLC wall-time
limit. The runner uses one worker, a 1 GiB Java heap, fixed fingerprint
polynomial zero, and a fresh run directory outside the checkout. Python's process
supervisor kills and waits for TLC when the wall-time limit expires. A timeout,
unrecognized verdict, or unexpected exit fails the command.

The historical receipts below used the [official TLA+ 1.7.4 release](https://github.com/tlaplus/tlaplus/releases/tag/v1.7.4),
with this SHA-256:

```text
936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88
```

The release's [execution environment manifest](https://github.com/tlaplus/tlaplus/blob/v1.7.4/tlatools/org.lamport.tlatools/META-INF/MANIFEST.MF)
lists Java 8/11 support; the runner requires Java 11+. It records the actual Java
version, TLC version, command, model/configuration digests, exit status, full
coverage output, and counterexample states in each `tlc.log`. No JAR or runtime
is committed. This release emits witness traces as text in the log.

## Recorded check

On 2026-09-15, TLC 2.19 (release 1.7.4, revision `5a47802`) on Temurin
17.0.20.1+1 completed the safety exploration in 3 seconds with **425,559 states
generated, 78,706 distinct states, zero queued states, and depth 23**.

| Configuration | Required result | Generated / distinct |
| --- | --- | --- |
| `Workspace.cfg` | All seven state invariants and both temporal safety properties pass | 425,559 / 78,706 |
| `StaleBase.cfg` | `NoStaleBaseCommit` fails with a nine-state trace: two disjoint edited paths publish in sequence despite the second proposal's older base | 6,322 / 1,771 |
| `StaleToken.cfg` | `NoStaleTokenRejection` fails with a six-state trace: reserve, activate, edit, release, reject retained stale edits | 144 / 51 |
| `BrokenFence.cfg` | Disabling the publication gate makes `AcceptedAuthority` fail with a six-state trace | 144 / 51 |

The last three runs intentionally stop at the required invariant violation; their
partial state counts are witness-search results, not completed safety checks.
The runner requires exit status 12, the exact named violation, and a trace for
each. The broken-fence run validates that removing the admission check is
observable by the selected safety property.

## Imported model coverage

The handoff's eleven selected cases were reproduced on 2026-09-15 with the
pinned checker. The current runner adds `EpochLogPending` and strengthens
EpochLog with `OnePending` and `PendingCleanView`.

| Configuration | Domain and limits | Checked result |
| --- | --- | --- |
| Core | 2 leaves, 2 keys, 2 values, token bound 3 | Reachable Inv and 3 action properties |
| CoreInductive1 | 2 leaves, 1 key, 2 values, token bound 2 | Inv from InvBounded |
| CoreInductive2 | 1 leaf, 2 keys, 2 values, token bound 2 | Inv from InvBounded |
| EpochLogTiny | 2 leaves, 1 path, 2 values, 2 epochs, token bound 2 | Invariants and 4 action properties |
| EpochLogPending | 1 leaf, 2 paths, 2 values, 2 epochs, token bound 1 | Invariants and 4 action properties |
| EpochLogWitness | 2 leaves, 2 paths, 2 values, 3 epochs, token bound 2 | Expected NoStaleBaseCommit violation |
| CowTreeSmoke | 2 leaves, 2 paths, 2 values, 2 nodes | Reachable Inv and 4 action properties |
| CowTreeInductive2 | 2 leaves, 1 path, 2 values, 2 nodes | Inv from finite Inv states |

All these configurations disable deadlock checking. Core, EpochLogTiny, and
CowTreeSmoke reduce leaf/path symmetry; values are not permuted because the
initial value is distinguished. Invariant-seeded and witness configurations
do not use symmetry; EpochLogPending does not either.

Core and EpochLog apply `TokenBound` as a state constraint. The finite
Core initial predicate bounds both token maps and fixes `merged = {}`.
The ghost is typed by `TypeOK`; its other possible initial histories are
not enumerated. The two selected runs reach depths 7 and 4, respectively,
not one layer. They check a finite exploration, not an unbounded induction
theorem. CowTree's selected invariant-seeded run covers 310,512 initial states.

### Pending proposal preservation

The imported EpochLog passed all its original checks while permitting:

    Fork -> RogueWrite(v1) -> Propose -> Sync

After Propose, dirty was empty. Sync then replaced the leaf's visible v1
with the old tip v0, even though its proposal still held v1. Reject would
return the path as dirty without restoring v1.

A second trace after fixing Sync exposed the related discard case:

    Fork -> RogueWrite(v1) -> Propose -> RogueWrite(v0) -> Discard

Discarding the later edit must restore the pending proposal's v1, not the
old tip. The current model protects pending paths during Sync and restores
the proposal's value and origin during Discard. Both preserve the ability
to edit after proposing. `PendingCleanView` checks the submitted value is
visible whenever that path has no newer local edit. `OnePending` checks
the uniqueness used by the discard lookup and batching argument.

The mounted runtime now replays selected histories through actual proposal,
sync, discard and publication operations; see `docs/verification.rst` for the
explicit mapping limits.

### Limits and larger configurations

`CoreInductive.cfg` and `CowTreeInductive1.cfg` have historical handoff
receipts but are excluded from the default budget. `EpochLogSmoke.cfg`,
`EpochLog.cfg`, `CowTree.cfg`, and `CowTreeInductive.cfg` have no completed
qualification in this integration. Do not cite a partial or killed run as
a pass. Run larger instances explicitly with a pinned checker and retain
their model/configuration bytes and verdicts.

Core's Grant/Commit combine service mutation and client reconciliation.
EpochLog separates proposal capture from commit, but still atomically combines
commit with acknowledgement and uses nondeterministic merge values.
CowTree has no integer fencing and retains global expiry/rogue ghosts.
These four historical models do not check storage recovery or garbage
collection. PublicationRecovery adds bounded process-failure ordering below.
Namespace semantics, hashing, actual durability guarantees, cache validity and
physical-space savings require separate implementation evidence.

Use `docs/verification.rst` for the implementation and proof requirements.

## Integration receipts (2026-09-15)

TLC 1.7.4 / TLC2 2.19, Temurin 17.0.20.1+1, one worker, 1 GiB heap.
All twelve selected configurations completed with the required verdict.
Safety runs exhausted their queues; exit-12 rows are expected witnesses.

| Configuration | Generated | Distinct | Exit |
| --- | ---: | ---: | ---: |
| Workspace | 425,559 | 78,706 | 0 |
| StaleBase | 6,322 | 1,771 | 12 |
| StaleToken | 144 | 51 | 12 |
| BrokenFence | 144 | 51 | 12 |
| Core | 2,248,391 | 160,803 | 0 |
| CoreInductive1 | 13,416 | 1,584 | 0 |
| CoreInductive2 | 62,160 | 8,464 | 0 |
| EpochLogTiny | 1,216,466 | 122,183 | 0 |
| EpochLogPending | 635,494 | 83,435 | 0 |
| EpochLogWitness | 332,553 | 80,262 | 12 |
| CowTreeSmoke | 3,110,358 | 251,776 | 0 |
| CowTreeInductive2 | 2,705,856 | 310,512 | 0 |

## Batch and replay coverage

[FencedPublish](FencedPublish.rst) adds seven bounded admission, induction,
witness and mutation cases. Its source and configurations remain unchanged.
The combined runner now checks these alongside the twelve handoff cases.

`EpochLogReplayDiscard` and `EpochLogReplayCommit` add two deterministic
executions of the actual EpochLog actions. Their final `ReplayIncomplete`
violations export the complete selected histories, while `Invariants` remains
enabled. Hash-bound native JSON fixtures include these two histories and
`EpochLogWitness`; mounted replay compares the observable state after each action.

All 21 configurations completed with the required verdict on 2026-09-17 UTC
using the current 1.8.0 pin. Their bounded domains are unchanged from the
historical cases; the two new wrappers each produce ten states. Larger
configurations and unbounded refinement remain unqualified.

## Process recovery boundaries

`PublicationRecovery.tla` supplements the atomic publication models with
Prepare, Build, Persist, MarkReady, Validate, Commit, Acknowledge, Crash,
Restart and GC. Its candidate/parent records, SQL commit and client validation
records follow the existing runtime boundaries. `docs/verification.rst` records
the abstraction assumptions and retained-window scope.

| Configuration | Required result | Generated / distinct |
| --- | --- | --- |
| PublicationRecovery | All invariants and LogAppendOnly, exhausted queue, depth 27 | 1,252,175 / 324,788 |
| PublicationWrongValidation | Exact ValidateBoundToId violation, exit 12 | 73,107 / 24,537 |
| PublicationPrematureGC | Exact AckedRecoverable violation, exit 12 | 13,979 / 5,373 |

These add three cases to the previous 21-case suite. Two writer keys, two
candidate IDs and token bound 2 define the complete configured domain. There
are no fairness assumptions or extra exploration constraints. The mutations
are controls in the model and are not supported runtime modes. Immutable object
closures and atomic durable SQL/client-record writes remain assumptions; this
is process-crash checking, not power-loss testing or an unbounded proof.
