--------------------------------- MODULE Core ---------------------------------
(***************************************************************************)
(* The smallest protocol under EpochLog and CowTree.                       *)
(*                                                                         *)
(* Service state is a value per key and a fencing token per key. There    *)
(* are two service operations: Grant bumps a key's token and returns the   *)
(* current value; Commit installs a delta if every key's presented token   *)
(* is current. That is all. Holders, expiry, staleness, proposals,         *)
(* epochs, forks, seals, batches, directories, and workspaces are either   *)
(* client-side state, ghosts, policy, or a choice of key space.            *)
(*                                                                         *)
(* One invariant carries the whole design:                                 *)
(*                                                                         *)
(*   Fenced: a leaf holding the current token for a key has origin equal   *)
(*           to the service value for that key.                            *)
(*                                                                         *)
(* With the structural conjuncts of Inv, a leaf whose dirty keys           *)
(* are all current commits cleanly, a leaf with any stale key cannot       *)
(* change the service at all, and a merge is ever needed only for a key    *)
(* written before its current token was obtained.                         *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets, TLC

CONSTANTS Leaves, Keys, Values, MaxToken

VARIABLES
  val,     \* [Keys -> Values]                 service: current value per key
  tok,     \* [Keys -> Nat]                    service: fencing token per key
  held,    \* [Leaves -> [Keys -> Nat]]        client: token the leaf was granted, 0 = none
  origin,  \* [Leaves -> [Keys -> Values]]     client: value returned by that grant (or last commit)
  view,    \* [Leaves -> [Keys -> Values]]     client: what the leaf would commit
  dirty,   \* [Leaves -> SUBSET Keys]          client: keys written since last grant/commit
  merged   \* ghost: keys where a merge was performed client-side

vars == << val, tok, held, origin, view, dirty, merged >>

v0 == CHOOSE v \in Values : TRUE
Symm == Permutations(Leaves) \union Permutations(Keys)

TypeOK ==
  /\ val \in [Keys -> Values]
  /\ tok \in [Keys -> Nat]
  /\ held \in [Leaves -> [Keys -> Nat]]
  /\ origin \in [Leaves -> [Keys -> Values]]
  /\ view \in [Leaves -> [Keys -> Values]]
  /\ dirty \in [Leaves -> SUBSET Keys]
  /\ merged \in SUBSET (Leaves \X Keys)

Init ==
  /\ val = [k \in Keys |-> v0]
  /\ tok = [k \in Keys |-> 0]
  /\ held = [l \in Leaves |-> [k \in Keys |-> 0]]
  /\ origin = [l \in Leaves |-> [k \in Keys |-> v0]]
  /\ view = [l \in Leaves |-> [k \in Keys |-> v0]]
  /\ dirty = [l \in Leaves |-> {}]
  /\ merged = {}

Current(l, k) == held[l][k] = tok[k] /\ held[l][k] # 0

-----------------------------------------------------------------------------
(* Grant: the service bumps the token and hands back the current value.    *)
(* No holder check: granting over a live holder is how expiry and stealing *)
(* are modeled; policy (TTLs, no-steal) lives outside the safety core.     *)
(* If the leaf had written the key before this grant, it reconciles        *)
(* locally: the new view is some function of (old origin, view, new value) *)
(* and the event is recorded. That function is the entire merge story.    *)
Grant(l, k) ==
  /\ tok' = [tok EXCEPT ![k] = tok[k] + 1]
  /\ held' = [held EXCEPT ![l][k] = tok[k] + 1]
  /\ origin' = [origin EXCEPT ![l][k] = val[k]]
  /\ IF k \in dirty[l] /\ origin[l][k] # val[k]
       THEN \E m \in Values :
              /\ view' = [view EXCEPT ![l][k] = m]
              /\ merged' = merged \union {<<l, k>>}
       ELSE /\ view' = [view EXCEPT ![l][k] = IF k \in dirty[l] THEN view[l][k] ELSE val[k]]
            /\ UNCHANGED merged
  /\ UNCHANGED << val, dirty >>

(* Write: purely local. No token needed; the token is checked at commit.   *)
Write(l, k, v) ==
  /\ v # view[l][k]
  /\ view' = [view EXCEPT ![l][k] = v]
  /\ dirty' = [dirty EXCEPT ![l] = dirty[l] \union {k}]
  /\ UNCHANGED << val, tok, held, origin, merged >>

(* Commit: install the dirty keys iff every presented token is current.    *)
(* Atomic. Touches exactly dirty[l] on the service side.                   *)
Commit(l) ==
  /\ dirty[l] # {}
  /\ \A k \in dirty[l] : Current(l, k)
  /\ val' = [k \in Keys |-> IF k \in dirty[l] THEN view[l][k] ELSE val[k]]
  /\ origin' = [origin EXCEPT ![l] = [k \in Keys |-> IF k \in dirty[l] THEN view[l][k] ELSE origin[l][k]]]
  /\ dirty' = [dirty EXCEPT ![l] = {}]
  /\ UNCHANGED << tok, held, view, merged >>

(* Sync: refresh non-dirty keys from the service. Local except for reads.  *)
Sync(l) ==
  /\ view' = [view EXCEPT ![l] = [k \in Keys |-> IF k \in dirty[l] THEN view[l][k] ELSE val[k]]]
  /\ origin' = [origin EXCEPT ![l] = [k \in Keys |-> IF k \in dirty[l] THEN origin[l][k] ELSE val[k]]]
  /\ UNCHANGED << val, tok, held, dirty, merged >>

(* Discard: throw away a local write. *)
Discard(l, k) ==
  /\ k \in dirty[l]
  /\ view' = [view EXCEPT ![l][k] = origin[l][k]]
  /\ dirty' = [dirty EXCEPT ![l] = dirty[l] \ {k}]
  /\ UNCHANGED << val, tok, held, origin, merged >>

Next ==
  \/ \E l \in Leaves, k \in Keys : Grant(l, k) \/ Discard(l, k)
  \/ \E l \in Leaves, k \in Keys, v \in Values : Write(l, k, v)
  \/ \E l \in Leaves : Commit(l) \/ Sync(l)

Spec == Init /\ [][Next]_vars

TokenBound == \A k \in Keys : tok[k] <= MaxToken

-----------------------------------------------------------------------------
(* THE invariant. *)
Fenced == \A l \in Leaves, k \in Keys : Current(l, k) => origin[l][k] = val[k]

(* At most one leaf is current on a key. Structural, but stated. *)
OneCurrent == \A l1, l2 \in Leaves, k \in Keys : Current(l1, k) /\ Current(l2, k) => l1 = l2

(* Tokens a leaf holds never exceed the service's. *)
HeldBounded == \A l \in Leaves, k \in Keys : held[l][k] <= tok[k]

Inv == TypeOK /\ Fenced /\ OneCurrent /\ HeldBounded

(* Same invariant with finite token ranges so TLC can enumerate it as an   *)
(* initial predicate for the inductiveness check (CoreInductive.cfg).      *)
TypeOKBounded ==
  /\ val \in [Keys -> Values]
  /\ tok \in [Keys -> 0..MaxToken]
  /\ held \in [Leaves -> [Keys -> 0..MaxToken]]
  /\ origin \in [Leaves -> [Keys -> Values]]
  /\ view \in [Leaves -> [Keys -> Values]]
  /\ dirty \in [Leaves -> SUBSET Keys]
  /\ merged = {}   \* TypeOK types this ghost; start with its empty history only

InvBounded == TypeOKBounded /\ Fenced /\ OneCurrent /\ HeldBounded


-----------------------------------------------------------------------------
(* Consequences, checked as action properties. *)

(* A commit changes exactly the committer's dirty keys; nothing else moves. *)
NoLostUpdate ==
  [][val' # val =>
       \E l \in Leaves : \A k \in Keys : (k \in dirty[l] => Current(l, k)) /\ (k \notin dirty[l] => val'[k] = val[k])]_vars

(* Every change to a key was made by the leaf currently holding its token, *)
(* with that leaf's own view. A stale token never changes anything.        *)
FencedOut ==
  [][\A k \in Keys : val'[k] # val[k] =>
       \E l \in Leaves : Current(l, k) /\ k \in dirty[l] /\ val'[k] = view[l][k]]_vars

(* THEOREM (as a consequence of Fenced): a merge happens only for a key    *)
(* that was written before its current token was obtained, i.e. never for *)
(* a leaf that grants before it writes. Stated via the ghost: a merge on   *)
(* (l,k) is recorded only during a Grant with k already dirty.             *)
MergesOnlyOnLateGrant ==
  [][\A m \in merged' \ merged : m[2] \in dirty[m[1]] /\ ~Current(m[1], m[2])]_vars

=============================================================================
