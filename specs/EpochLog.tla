------------------------------- MODULE EpochLog -------------------------------
(***************************************************************************)
(* The distributed formulation of the snapshot tree.                       *)
(*                                                                         *)
(* A workspace is a linearizable, append-only log of immutable epochs      *)
(* E_1, E_2, ... . Agents read an epoch (point in time, free: it is an     *)
(* immutable value), take path leases carrying fencing tokens, write in a  *)
(* private leaf, and propose a delta. A commit takes a batch of valid      *)
(* proposals and appends E_{t+1} = E_t overlaid with their deltas. Every   *)
(* other path of E_{t+1} equals E_t.                                       *)
(*                                                                         *)
(* Reads of retained epochs are immutable; reads of the current tip and   *)
(* lease changes require the coordinator's ordered state.                 *)
(* Append and lease changes are atomic here. Leases are checked by fencing *)
(* tokens at commit, never by clocks, so safety never depends on time and  *)
(* only liveness does.                                                     *)
(*                                                                         *)
(* Theorem (checked): with no lease expiries and no unleased writes, no    *)
(* commit ever needs a merge, for every epoch up to the bound, by          *)
(* induction on the log.                                                   *)
(*                                                                         *)
(* Witness (checked by negation, see EpochLogWitness.cfg): a proposal      *)
(* whose base epoch is stale still commits without conflict. Freshness is  *)
(* not a requirement; leases are.                                          *)
(*                                                                         *)
(* Provenance: this module arrived in the working container from a sibling *)
(* session during the 2026-09-15 design work and was verified here (tiny   *)
(* and witness configs). It supersedes CowTree.tla's boolean `expired`     *)
(* with integer fencing tokens and splits publish into Propose/Commit.     *)
(***************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets, TLC

CONSTANTS Leaves, Paths, Values, MaxEpochs, MaxToken, None

ASSUME None \notin Leaves /\ None \notin Values /\ MaxEpochs \in Nat /\ MaxEpochs >= 1

VARIABLES
  log,        \* Seq of epochs, each [Paths -> Values]; immutable, append only
  token,      \* [Paths -> Nat]  fencing token, bumped on every grant
  holder,     \* [Paths -> Leaves \cup {None}]  who the service thinks holds the lease
  active,     \* [Leaves -> BOOLEAN]
  held,       \* [Leaves -> [Paths -> Nat]]  token the leaf believes it holds, 0 = none
  view,       \* [Leaves -> [Paths -> Values]]
  origin,     \* [Leaves -> [Paths -> Values]]  the tip value each path was last synced from
  readEpoch,  \* [Leaves -> Nat]  the epoch the leaf's unsynced paths reflect
  dirty,      \* [Leaves -> SUBSET Paths]
  stale,      \* [Leaves -> SUBSET Paths]  re-leased over dirty writes; merge may be needed
  proposals,  \* set of pending proposals (records)
  expired,    \* ghost: a lease was force-expired at some point
  rogue,      \* ghost: a write without ever holding a lease happened at some point
  merges,     \* ghost: {<<epoch index, path>>} that needed a three-way merge
  staleBaseCommitted  \* ghost: a proposal with a stale base epoch was committed

vars == << log, token, holder, active, held, view, origin, readEpoch, dirty, stale,
           proposals, expired, rogue, merges, staleBaseCommitted >>

v0 == CHOOSE v \in Values : TRUE
Tip == log[Len(log)]
T == Len(log)
OptValues == Values \cup {None}

Proposal == [leaf: Leaves,
             delta: [Paths -> OptValues],
             tokens: [Paths -> Nat],
             origin: [Paths -> Values],
             base: Nat,
             stale: SUBSET Paths]

Dom(pr) == {p \in Paths : pr.delta[p] # None}
Pending(l) == {pr \in proposals : pr.leaf = l}
PendingPaths(l) == UNION {Dom(pr) : pr \in Pending(l)}
StaleFor(l) == stale[l] \cup UNION {pr.stale : pr \in Pending(l)}

Symm == Permutations(Leaves) \union Permutations(Paths)

TypeOK ==
  /\ log \in Seq([Paths -> Values]) /\ Len(log) \in 1..MaxEpochs
  /\ token \in [Paths -> Nat]
  /\ holder \in [Paths -> Leaves \cup {None}]
  /\ active \in [Leaves -> BOOLEAN]
  /\ held \in [Leaves -> [Paths -> Nat]]
  /\ view \in [Leaves -> [Paths -> Values]]
  /\ origin \in [Leaves -> [Paths -> Values]]
  /\ readEpoch \in [Leaves -> Nat]
  /\ dirty \in [Leaves -> SUBSET Paths]
  /\ stale \in [Leaves -> SUBSET Paths]
  /\ proposals \subseteq Proposal
  /\ expired \in BOOLEAN /\ rogue \in BOOLEAN /\ staleBaseCommitted \in BOOLEAN
  /\ merges \subseteq (1..MaxEpochs) \X Paths

Init ==
  /\ log = << [p \in Paths |-> v0] >>
  /\ token = [p \in Paths |-> 0]
  /\ holder = [p \in Paths |-> None]
  /\ active = [l \in Leaves |-> FALSE]
  /\ held = [l \in Leaves |-> [p \in Paths |-> 0]]
  /\ view = [l \in Leaves |-> [p \in Paths |-> v0]]
  /\ origin = [l \in Leaves |-> [p \in Paths |-> v0]]
  /\ readEpoch = [l \in Leaves |-> 1]
  /\ dirty = [l \in Leaves |-> {}]
  /\ stale = [l \in Leaves |-> {}]
  /\ proposals = {}
  /\ expired = FALSE /\ rogue = FALSE /\ staleBaseCommitted = FALSE
  /\ merges = {}

-----------------------------------------------------------------------------
(* Fork: read any epoch, point in time. No coordination: epochs are values. *)
Fork(l, t) ==
  /\ ~active[l] /\ t \in 1..T
  /\ active' = [active EXCEPT ![l] = TRUE]
  /\ view' = [view EXCEPT ![l] = log[t]]
  /\ origin' = [origin EXCEPT ![l] = log[t]]
  /\ readEpoch' = [readEpoch EXCEPT ![l] = t]
  /\ held' = [held EXCEPT ![l] = [p \in Paths |-> 0]]
  /\ dirty' = [dirty EXCEPT ![l] = {}]
  /\ stale' = [stale EXCEPT ![l] = {}]
  /\ UNCHANGED << log, token, holder, proposals, expired, rogue, merges, staleBaseCommitted >>

(* Drop: discard the leaf, its leases, and its in-flight proposals. TLC found *)
(* the proposal part: a rejected proposal must not bounce to a slot's next    *)
(* incarnation.                                                               *)
Drop(l) ==
  /\ active[l]
  /\ active' = [active EXCEPT ![l] = FALSE]
  /\ holder' = [p \in Paths |-> IF holder[p] = l THEN None ELSE holder[p]]
  /\ held' = [held EXCEPT ![l] = [p \in Paths |-> 0]]
  /\ dirty' = [dirty EXCEPT ![l] = {}]
  /\ stale' = [stale EXCEPT ![l] = {}]
  /\ proposals' = proposals \ Pending(l)
  /\ UNCHANGED << log, token, view, origin, readEpoch, expired, rogue, merges, staleBaseCommitted >>

(* Acquire: grant a fresh fencing token and sync the path from the tip. *)
Acquire(l, p) ==
  /\ active[l] /\ holder[p] = None /\ p \notin dirty[l]
  /\ \A pr \in Pending(l) : p \notin Dom(pr)
  /\ token' = [token EXCEPT ![p] = token[p] + 1]
  /\ holder' = [holder EXCEPT ![p] = l]
  /\ held' = [held EXCEPT ![l][p] = token[p] + 1]
  /\ view' = [view EXCEPT ![l][p] = Tip[p]]
  /\ origin' = [origin EXCEPT ![l][p] = Tip[p]]
  /\ UNCHANGED << log, active, readEpoch, dirty, stale, proposals, expired, rogue, merges, staleBaseCommitted >>

Release(l, p) ==
  /\ holder[p] = l /\ p \notin dirty[l]
  /\ \A pr \in Pending(l) : p \notin Dom(pr)
  /\ holder' = [holder EXCEPT ![p] = None]
  /\ held' = [held EXCEPT ![l][p] = 0]
  /\ UNCHANGED << log, token, active, view, origin, readEpoch, dirty, stale, proposals, expired, rogue, merges, staleBaseCommitted >>

(* Write: the leaf believes it holds the lease. After an expiry that belief *)
(* can be wrong; fencing at commit is what protects the log, not the belief. *)
Write(l, p, v) ==
  /\ active[l] /\ held[l][p] # 0 /\ v # view[l][p]
  /\ view' = [view EXCEPT ![l][p] = v]
  /\ dirty' = [dirty EXCEPT ![l] = dirty[l] \cup {p}]
  /\ UNCHANGED << log, token, holder, active, held, origin, readEpoch, stale, proposals, expired, rogue, merges, staleBaseCommitted >>

(* RogueWrite: never took a lease at all. *)
RogueWrite(l, p, v) ==
  /\ active[l] /\ held[l][p] = 0 /\ v # view[l][p]
  /\ view' = [view EXCEPT ![l][p] = v]
  /\ dirty' = [dirty EXCEPT ![l] = dirty[l] \cup {p}]
  /\ rogue' = TRUE
  /\ UNCHANGED << log, token, holder, active, held, origin, readEpoch, stale, proposals, expired, merges, staleBaseCommitted >>

(* Submitted paths remain local until Commit or Reject reconciles them. *)
Sync(l) ==
  /\ active[l]
  /\ view' = [view EXCEPT ![l] = [p \in Paths |-> IF p \in dirty[l] \cup PendingPaths(l) THEN view[l][p] ELSE Tip[p]]]
  /\ origin' = [origin EXCEPT ![l] = [p \in Paths |-> IF p \in dirty[l] \cup PendingPaths(l) THEN origin[l][p] ELSE Tip[p]]]
  /\ readEpoch' = [readEpoch EXCEPT ![l] = T]
  /\ UNCHANGED << log, token, holder, active, held, dirty, stale, proposals, expired, rogue, merges, staleBaseCommitted >>

(* Propose: submit the dirty paths with the tokens the leaf believes it holds. *)
(* One proposal in flight per leaf keeps the model bounded.                   *)
Propose(l) ==
  /\ active[l] /\ dirty[l] # {} /\ Pending(l) = {}
  /\ proposals' = proposals \cup
       {[leaf |-> l,
         delta |-> [p \in Paths |-> IF p \in dirty[l] THEN view[l][p] ELSE None],
         tokens |-> held[l],
         origin |-> origin[l],
         base |-> readEpoch[l],
         stale |-> stale[l]]}
  /\ dirty' = [dirty EXCEPT ![l] = {}]
  /\ stale' = [stale EXCEPT ![l] = {}]
  /\ UNCHANGED << log, token, holder, active, held, view, origin, readEpoch, expired, rogue, merges, staleBaseCommitted >>

(* Fencing: every path of the proposal is held by its leaf with the current token. *)
Valid(pr) == \A p \in Dom(pr) : holder[p] = pr.leaf /\ pr.tokens[p] = token[p]

Contested(pr) == {p \in Dom(pr) : Tip[p] # pr.origin[p]}

(* Commit: append E_{t+1} from a batch of valid proposals. Disjointness     *)
(* relies on one holder per path AND one pending proposal per leaf.        *)
(* Contested                                                              *)
(* paths get a nondeterministic merged value standing in for three-way      *)
(* merge, rerere, or a resolver agent, and are recorded.                    *)
Commit(B) ==
  /\ B # {} /\ B \subseteq {pr \in proposals : Valid(pr)}
  /\ T < MaxEpochs
  /\ LET contestedAll == UNION {Contested(pr) : pr \in B}
         writers == UNION {Dom(pr) : pr \in B}
     IN \E merged \in [contestedAll -> Values] :
       LET e == [p \in Paths |->
                   IF p \in contestedAll THEN merged[p]
                   ELSE IF p \in writers THEN (CHOOSE pr \in B : p \in Dom(pr)).delta[p]
                   ELSE Tip[p]]
           committedLeaves == {pr.leaf : pr \in B}
           Proposed(l, p) == \E pr \in B : pr.leaf = l /\ p \in Dom(pr)
       IN /\ log' = Append(log, e)
          /\ proposals' = proposals \ B
          (* A write made after proposing sits on top of the proposed value, which is now the tip: *)
          (* keep the newer view, but its origin becomes the committed value. TLC found this one.  *)
          /\ view' = [l \in Leaves |-> IF l \in committedLeaves
                                       THEN [p \in Paths |-> IF p \in dirty[l] THEN view[l][p] ELSE e[p]]
                                       ELSE view[l]]
          /\ origin' = [l \in Leaves |-> IF l \in committedLeaves
                                         THEN [p \in Paths |-> IF p \in dirty[l] /\ ~Proposed(l, p) THEN origin[l][p] ELSE e[p]]
                                         ELSE origin[l]]
          /\ readEpoch' = [l \in Leaves |-> IF l \in committedLeaves THEN T + 1 ELSE readEpoch[l]]
          /\ merges' = merges \cup {<<T + 1, p>> : p \in contestedAll}
          /\ staleBaseCommitted' = (staleBaseCommitted \/ \E pr \in B : pr.base < T)
  /\ UNCHANGED << token, holder, active, held, dirty, stale, expired, rogue >>

(* Reject: a proposal whose tokens are no longer current is bounced back to  *)
(* its leaf as dirty, stale paths. The leaf must Reacquire or Discard them.  *)
Reject(pr) ==
  /\ pr \in proposals /\ ~Valid(pr) /\ active[pr.leaf]
  /\ proposals' = proposals \ {pr}
  /\ dirty' = [dirty EXCEPT ![pr.leaf] = dirty[pr.leaf] \cup Dom(pr)]
  /\ stale' = [stale EXCEPT ![pr.leaf] = stale[pr.leaf] \cup Dom(pr) \cup pr.stale]
  /\ held' = [held EXCEPT ![pr.leaf] = [p \in Paths |-> IF p \in Dom(pr) /\ holder[p] # pr.leaf THEN 0 ELSE held[pr.leaf][p]]]
  /\ UNCHANGED << log, token, holder, active, view, origin, readEpoch, expired, rogue, merges, staleBaseCommitted >>

(* Expire: timeout or crash, as seen by the lease service. The holder keeps *)
(* believing; its token is now stale.                                       *)
Expire(p) ==
  /\ holder[p] # None
  /\ holder' = [holder EXCEPT ![p] = None]
  /\ expired' = TRUE
  /\ UNCHANGED << log, token, active, held, view, origin, readEpoch, dirty, stale, proposals, rogue, merges, staleBaseCommitted >>

(* Reacquire: take a lease over a path the leaf already dirtied. Cannot sync, *)
(* so the path is stale and may need a merge at commit.                       *)
Reacquire(l, p) ==
  /\ active[l] /\ holder[p] = None /\ p \in dirty[l]
  /\ token' = [token EXCEPT ![p] = token[p] + 1]
  /\ holder' = [holder EXCEPT ![p] = l]
  /\ held' = [held EXCEPT ![l][p] = token[p] + 1]
  /\ stale' = [stale EXCEPT ![l] = stale[l] \cup {p}]
  /\ UNCHANGED << log, active, view, origin, readEpoch, dirty, proposals, expired, rogue, merges, staleBaseCommitted >>

Discard(l, p) ==
  /\ active[l] /\ p \in dirty[l] /\ holder[p] # l
  /\ LET pending == {pr \in Pending(l) : p \in Dom(pr)}
         pr == CHOOSE candidate \in pending : TRUE
     IN /\ view' = [view EXCEPT ![l][p] = IF pending = {} THEN Tip[p] ELSE pr.delta[p]]
        /\ origin' = [origin EXCEPT ![l][p] = IF pending = {} THEN Tip[p] ELSE pr.origin[p]]
  /\ dirty' = [dirty EXCEPT ![l] = dirty[l] \ {p}]
  /\ stale' = [stale EXCEPT ![l] = stale[l] \ {p}]
  /\ held' = [held EXCEPT ![l][p] = 0]
  /\ UNCHANGED << log, token, holder, active, readEpoch, proposals, expired, rogue, merges, staleBaseCommitted >>

Next ==
  \/ \E l \in Leaves, t \in 1..T : Fork(l, t)
  \/ \E l \in Leaves : Drop(l) \/ Sync(l) \/ Propose(l)
  \/ \E l \in Leaves, p \in Paths : Acquire(l, p) \/ Release(l, p) \/ Reacquire(l, p) \/ Discard(l, p)
  \/ \E l \in Leaves, p \in Paths, v \in Values : Write(l, p, v) \/ RogueWrite(l, p, v)
  \/ \E B \in SUBSET proposals : Commit(B)
  \/ \E pr \in proposals : Reject(pr)
  \/ \E p \in Paths : Expire(p)

Spec == Init /\ [][Next]_vars

TokenBound == \A p \in Paths : token[p] <= MaxToken

-----------------------------------------------------------------------------
(* Invariants *)

(* Without expiries, what a leaf believes about its leases is true. *)
BeliefsAccurate ==
  ~expired => \A l \in Leaves, p \in Paths :
    held[l][p] # 0 => (holder[p] = l /\ held[l][p] = token[p])

(* The inductive heart, now over the log: a held, non-stale path is in sync *)
(* with the current tip, whatever epoch the leaf originally read.           *)
LeasedPathsSynced ==
  \A p \in Paths : (holder[p] # None /\ p \notin StaleFor(holder[p])) =>
    origin[holder[p]][p] = Tip[p]

StaleOnlyAfterExpiryOrRogue ==
  (~expired /\ ~rogue) => \A l \in Leaves : StaleFor(l) = {}

(* THEOREM, for every epoch up to the bound. *)
ConflictFreeWithoutExpiryOrRogue == (~expired /\ ~rogue) => merges = {}

(* A submitted value remains visible until acknowledged, rejected, or edited. *)
OnePending == \A l \in Leaves : Cardinality(Pending(l)) <= 1

PendingCleanView ==
  \A pr \in proposals : \A p \in Dom(pr) \ dirty[pr.leaf] :
    view[pr.leaf][p] = pr.delta[p]

Invariants ==
  /\ TypeOK
  /\ BeliefsAccurate
  /\ LeasedPathsSynced
  /\ StaleOnlyAfterExpiryOrRogue
  /\ ConflictFreeWithoutExpiryOrRogue
  /\ OnePending
  /\ PendingCleanView

(* Negate this in EpochLogWitness.cfg: TLC's counterexample is an execution *)
(* in which a proposal with a stale base epoch commits without a merge.     *)
NoStaleBaseCommit == ~staleBaseCommitted

-----------------------------------------------------------------------------
(* Action properties *)

(* The log is linearizable and immutable: it only ever grows by one epoch,  *)
(* and existing epochs never change.                                        *)
LogAppendOnly ==
  [][/\ Len(log') \in {Len(log), Len(log) + 1}
     /\ \A t \in 1..Len(log) : log'[t] = log[t]]_vars

(* NoLostUpdate with fencing: every path that changes between E_t and       *)
(* E_{t+1} was written by a proposal from its current lease holder carrying *)
(* the current token; every other path is carried over unchanged.           *)
NoLostUpdate ==
  [][Len(log') = Len(log) + 1 =>
       \A p \in Paths : log'[Len(log) + 1][p] # log[Len(log)][p] =>
         \E pr \in proposals \ proposals' :
           /\ p \in Dom(pr)
           /\ holder[p] = pr.leaf
           /\ pr.tokens[p] = token[p]]_vars

(* A proposal carrying a stale token never changes the log: the only way an *)
(* invalid proposal leaves the set is Reject, which appends nothing.         *)
FencedOut ==
  [][\A pr \in proposals \ proposals' : ~Valid(pr) => Len(log') = Len(log)]_vars

MergesOnlyOnStale ==
  [][\A m \in merges' \ merges :
       \E pr \in proposals \ proposals' : m[2] \in pr.stale]_vars

=============================================================================
