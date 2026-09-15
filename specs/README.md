# Workspace protocol model

`Workspace.tla` is a new, bounded review model for the proposed workspace
protocol. It does not describe the current worktree CLI implementation.
The original `CowTree.tla` and `EpochLog.tla` mentioned in the design notes were
not supplied. Their reported state counts and induction checks are unverified.

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

No implementation replay or refinement proof connects this model to executable
metadata operations. Those operations have not been implemented here.

## Reproduce

Install Java 11 or later and `uv`, then run from the checkout:

```sh
uv run python scripts/check_specs.py --cache ../cowtree-tla-cache
```

Use `--java /absolute/path/to/java` to select a runtime, `--case Workspace` to
select one configuration, or `--timeout-seconds 180` to set each TLC wall-time
limit. The runner uses one worker, a 512 MiB Java heap, fixed fingerprint
polynomial zero, and a fresh run directory outside the checkout. Python's process
supervisor kills and waits for TLC when the wall-time limit expires. A timeout,
unrecognized verdict, or unexpected exit fails the command.

The runner downloads the [official TLA+ 1.7.4 release](https://github.com/tlaplus/tlaplus/releases/tag/v1.7.4),
checks its SHA-256, and exports `TLA2TOOLS_JAR` for the verified artifact:

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
