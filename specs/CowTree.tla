------------------------------- MODULE CowTree -------------------------------
(***************************************************************************)
(* A snapshot tree: the "inverted worktree".                               *)
(*                                                                         *)
(* Nodes are immutable filesystem snapshots. A leaf is a mutable view      *)
(* forked from a node, with exactly one writer. A workspace has a tip      *)
(* node that only moves forward. Agents take exclusive leases on paths,    *)
(* write in their own leaf, and publish: publishing builds a new node from *)
(* the current tip plus the leaf's leased, dirty paths, and swings the     *)
(* tip to it.                                                              *)
(*                                                                         *)
(* The theorem this spec exists to check:                                  *)
(*                                                                         *)
(*   leases + sync-on-acquire  ==>  every publish is conflict free.        *)
(*                                                                         *)
(* Three-way merging is therefore a recovery path (after a lease was       *)
(* force-expired, i.e. a crash or timeout), never the steady state.        *)
(*                                                                         *)
(* Multi-writer coherence is deliberately absent: concurrency comes from   *)
(* many leaves, never from many writers on one leaf.                       *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets, TLC

CONSTANTS Leaves,     \* leaf slots (each has one implicit writer)
          Paths,      \* file paths
          Values,     \* file contents
          MaxNodes,   \* state-space bound on snapshots
          None        \* model value: "no lease" / "no value"

ASSUME None \notin Leaves /\ None \notin Values /\ MaxNodes \in Nat /\ MaxNodes >= 1

VARIABLES
  nodeCount,  \* nodes are 1..nodeCount, created in order
  content,    \* [1..nodeCount -> [Paths -> Values]]   immutable once created
  parent,     \* [1..nodeCount -> 0..MaxNodes]        0 = root
  tip,        \* the workspace tip, in 1..nodeCount
  active,     \* [Leaves -> BOOLEAN]                  slot in use
  base,       \* [Leaves -> 1..MaxNodes]              node the leaf was forked from
  view,       \* [Leaves -> [Paths -> Values]]        what the writer sees
  origin,     \* [Leaves -> [Paths -> Values]]        tip value each path was last synced from
  dirty,      \* [Leaves -> SUBSET Paths]             written since last sync/publish
  stale,      \* [Leaves -> SUBSET Paths]             re-leased after expiry without sync
  lease,      \* [Paths -> Leaves \cup {None}]        exclusive path leases
  expired,    \* ghost: some lease was force-expired (crash/timeout) at some point
  rogue,      \* ghost: some write happened without a lease at some point
  merges      \* ghost: set of (node, path) that needed a three-way merge on publish

vars == << nodeCount, content, parent, tip, active, base, view, origin,
           dirty, stale, lease, expired, rogue, merges >>

Nodes == 1..nodeCount
v0 == CHOOSE v \in Values : TRUE

(* Leaves and paths are interchangeable; values are not (v0 is the initial content). *)
Symm == Permutations(Leaves) \union Permutations(Paths)

TypeOK ==
  /\ nodeCount \in 1..MaxNodes
  /\ content \in [Nodes -> [Paths -> Values]]
  /\ parent \in [Nodes -> 0..MaxNodes]
  /\ tip \in Nodes
  /\ active \in [Leaves -> BOOLEAN]
  /\ base \in [Leaves -> 1..MaxNodes]
  /\ view \in [Leaves -> [Paths -> Values]]
  /\ origin \in [Leaves -> [Paths -> Values]]
  /\ dirty \in [Leaves -> SUBSET Paths]
  /\ stale \in [Leaves -> SUBSET Paths]
  /\ lease \in [Paths -> Leaves \cup {None}]
  /\ expired \in BOOLEAN
  /\ rogue \in BOOLEAN
  /\ merges \in SUBSET ((1..MaxNodes) \X Paths)

Init ==
  /\ nodeCount = 1
  /\ content = [n \in {1} |-> [p \in Paths |-> v0]]
  /\ parent = [n \in {1} |-> 0]
  /\ tip = 1
  /\ active = [l \in Leaves |-> FALSE]
  /\ base = [l \in Leaves |-> 1]
  /\ view = [l \in Leaves |-> [p \in Paths |-> v0]]
  /\ origin = [l \in Leaves |-> [p \in Paths |-> v0]]
  /\ dirty = [l \in Leaves |-> {}]
  /\ stale = [l \in Leaves |-> {}]
  /\ lease = [p \in Paths |-> None]
  /\ expired = FALSE
  /\ rogue = FALSE
  /\ merges = {}

(* Append a node whose parent is `par` with content `c`. *)
NewNode(par, c) ==
  /\ nodeCount < MaxNodes
  /\ nodeCount' = nodeCount + 1
  /\ content' = [n \in 1..nodeCount + 1 |-> IF n = nodeCount + 1 THEN c ELSE content[n]]
  /\ parent' = [n \in 1..nodeCount + 1 |-> IF n = nodeCount + 1 THEN par ELSE parent[n]]

-----------------------------------------------------------------------------
(* Fork: a free leaf slot takes an O(1) mutable view of any existing node. *)
Fork(l, n) ==
  /\ ~active[l]
  /\ n \in Nodes
  /\ active' = [active EXCEPT ![l] = TRUE]
  /\ base' = [base EXCEPT ![l] = n]
  /\ view' = [view EXCEPT ![l] = content[n]]
  /\ origin' = [origin EXCEPT ![l] = content[n]]
  /\ dirty' = [dirty EXCEPT ![l] = {}]
  /\ stale' = [stale EXCEPT ![l] = {}]
  /\ UNCHANGED << nodeCount, content, parent, tip, lease, expired, rogue, merges >>

(* Drop: discard a leaf and every lease it holds. Unpublished work is lost, *)
(* which is fine: it was never visible to anyone else.                      *)
Drop(l) ==
  /\ active[l]
  /\ active' = [active EXCEPT ![l] = FALSE]
  /\ dirty' = [dirty EXCEPT ![l] = {}]
  /\ stale' = [stale EXCEPT ![l] = {}]
  /\ lease' = [p \in Paths |-> IF lease[p] = l THEN None ELSE lease[p]]
  /\ UNCHANGED << nodeCount, content, parent, tip, base, view, origin, expired, rogue, merges >>

(* Acquire: exclusive lease on a path. Sync-on-acquire installs the tip's *)
(* current value for that path into the leaf, which is safe because the    *)
(* leaf has no unleased write on it (see RogueWrite for why that matters). *)
Acquire(l, p) ==
  /\ active[l]
  /\ lease[p] = None
  /\ p \notin dirty[l]
  /\ lease' = [lease EXCEPT ![p] = l]
  /\ view' = [view EXCEPT ![l][p] = content[tip][p]]
  /\ origin' = [origin EXCEPT ![l][p] = content[tip][p]]
  /\ UNCHANGED << nodeCount, content, parent, tip, active, base, dirty, stale, expired, rogue, merges >>

(* Release: give a lease back. Only after the writes under it were published. *)
Release(l, p) ==
  /\ lease[p] = l
  /\ p \notin dirty[l]
  /\ lease' = [lease EXCEPT ![p] = None]
  /\ UNCHANGED << nodeCount, content, parent, tip, active, base, view, origin, dirty, stale, expired, rogue, merges >>

(* Write: the leaf's writer changes a leased path. *)
Write(l, p, v) ==
  /\ active[l]
  /\ lease[p] = l
  /\ v # view[l][p]
  /\ view' = [view EXCEPT ![l][p] = v]
  /\ dirty' = [dirty EXCEPT ![l] = dirty[l] \cup {p}]
  /\ UNCHANGED << nodeCount, content, parent, tip, active, base, origin, stale, lease, expired, rogue, merges >>

(* RogueWrite: a write without a lease. A commodity filesystem cannot stop *)
(* this; the design only promises it can never reach the tip unfenced.     *)
RogueWrite(l, p, v) ==
  /\ active[l]
  /\ lease[p] # l
  /\ v # view[l][p]
  /\ view' = [view EXCEPT ![l][p] = v]
  /\ dirty' = [dirty EXCEPT ![l] = dirty[l] \cup {p}]
  /\ rogue' = TRUE
  /\ UNCHANGED << nodeCount, content, parent, tip, active, base, origin, stale, lease, expired, merges >>

(* Sync: pull the tip into every path the leaf has not written. This is the *)
(* "live rebase" that lets an agent see everyone else's published work.     *)
Sync(l) ==
  /\ active[l]
  /\ view' = [view EXCEPT ![l] = [p \in Paths |-> IF p \in dirty[l] THEN view[l][p] ELSE content[tip][p]]]
  /\ origin' = [origin EXCEPT ![l] = [p \in Paths |-> IF p \in dirty[l] THEN origin[l][p] ELSE content[tip][p]]]
  /\ UNCHANGED << nodeCount, content, parent, tip, active, base, dirty, stale, lease, expired, rogue, merges >>

(* Seal: freeze the leaf's current view as a private node under its base.  *)
(* This is what makes fork-of-fork cheap: another leaf can Fork this node.  *)
(* Note the sealed node may carry unpublished values; a forker inherits     *)
(* them as read-only context, not as ownership (it holds no lease).         *)
Seal(l) ==
  /\ active[l]
  /\ NewNode(base[l], view[l])
  /\ base' = [base EXCEPT ![l] = nodeCount + 1]
  /\ UNCHANGED << tip, active, view, origin, dirty, stale, lease, expired, rogue, merges >>

(* Publish: build tip' = tip overlaid with the leaf's dirty paths, all of   *)
(* which must be leased by this leaf (fencing). Paths the leaf did not      *)
(* write come from the tip, never from the leaf, so a stale leaf cannot     *)
(* clobber anyone. A dirty path whose origin no longer matches the tip is   *)
(* contested; its merged value is chosen nondeterministically here, which  *)
(* stands in for a three-way merge, and the event is recorded in `merges`. *)
Publish(l) ==
  /\ active[l]
  /\ dirty[l] # {}
  /\ \A p \in dirty[l] : lease[p] = l
  /\ LET contested == {p \in dirty[l] : content[tip][p] # origin[l][p]}
     IN \E merged \in [contested -> Values] :
       LET c == [p \in Paths |->
                   IF p \in contested THEN merged[p]
                   ELSE IF p \in dirty[l] THEN view[l][p]
                   ELSE content[tip][p]]
       IN /\ NewNode(tip, c)
          /\ tip' = nodeCount + 1
          /\ base' = [base EXCEPT ![l] = nodeCount + 1]
          /\ view' = [view EXCEPT ![l] = c]
          /\ origin' = [origin EXCEPT ![l] = c]
          /\ dirty' = [dirty EXCEPT ![l] = {}]
          /\ stale' = [stale EXCEPT ![l] = {}]
          /\ merges' = merges \cup {<<nodeCount + 1, p>> : p \in contested}
  /\ UNCHANGED << active, lease, expired, rogue >>

(* Expire: the lease service times out or the holder crashed. *)
Expire(p) ==
  /\ lease[p] # None
  /\ lease' = [lease EXCEPT ![p] = None]
  /\ expired' = TRUE
  /\ UNCHANGED << nodeCount, content, parent, tip, active, base, view, origin, dirty, stale, rogue, merges >>

(* Reacquire: a leaf holding unpublished writes on an unleased path (its    *)
(* lease expired, or it wrote without one) takes the lease without syncing  *)
(* (it cannot: the path is dirty). The path is marked stale, and its        *)
(* publish may need a merge.                                                *)
Reacquire(l, p) ==
  /\ active[l]
  /\ lease[p] = None
  /\ p \in dirty[l]
  /\ lease' = [lease EXCEPT ![p] = l]
  /\ stale' = [stale EXCEPT ![l] = stale[l] \cup {p}]
  /\ UNCHANGED << nodeCount, content, parent, tip, active, base, view, origin, dirty, expired, rogue, merges >>

(* Discard: drop an unleased dirty write and resync the path from the tip. *)
Discard(l, p) ==
  /\ active[l]
  /\ p \in dirty[l]
  /\ lease[p] # l
  /\ view' = [view EXCEPT ![l][p] = content[tip][p]]
  /\ origin' = [origin EXCEPT ![l][p] = content[tip][p]]
  /\ dirty' = [dirty EXCEPT ![l] = dirty[l] \ {p}]
  /\ stale' = [stale EXCEPT ![l] = stale[l] \ {p}]
  /\ UNCHANGED << nodeCount, content, parent, tip, active, base, lease, expired, rogue, merges >>

Next ==
  \/ \E l \in Leaves, n \in Nodes : Fork(l, n)
  \/ \E l \in Leaves : Drop(l) \/ Sync(l) \/ Seal(l) \/ Publish(l)
  \/ \E l \in Leaves, p \in Paths : Acquire(l, p) \/ Release(l, p) \/ Reacquire(l, p) \/ Discard(l, p)
  \/ \E l \in Leaves, p \in Paths, v \in Values : Write(l, p, v) \/ RogueWrite(l, p, v)
  \/ \E p \in Paths : Expire(p)

Spec == Init /\ [][Next]_vars

-----------------------------------------------------------------------------
(* Invariants *)

(* I2: parents exist and precede their children, so the node graph is a tree. *)
Acyclic == \A n \in Nodes : parent[n] < n

(* I4/I5: leases are exclusive by construction; the leased dirty sets of two *)
(* leaves are therefore disjoint.                                             *)
DisjointLeasedDirty ==
  \A l1, l2 \in Leaves : l1 # l2 =>
    {p \in dirty[l1] : lease[p] = l1} \cap {p \in dirty[l2] : lease[p] = l2} = {}

(* The inductive heart: a leased, non-stale path in a leaf is always in sync *)
(* with the tip, because only that leaf can publish it.                      *)
LeasedPathsSynced ==
  \A p \in Paths : (lease[p] # None /\ p \notin stale[lease[p]]) =>
    origin[lease[p]][p] = content[tip][p]

(* Stale paths only exist after an expiry or a write made without a lease. *)
StaleOnlyAfterExpiry == (~expired /\ ~rogue) => \A l \in Leaves : stale[l] = {}

(* THEOREM: with leases honored and never force-expired, no publish ever *)
(* needed a merge. Conflicts come from crashes or discipline violations,  *)
(* never from ordinary parallel work.                                     *)
ConflictFreeWithoutExpiry == (~expired /\ ~rogue) => merges = {}

(* Even with expiries, merges are confined to paths that were stale. Recorded *)
(* as an action property below (MergesOnlyOnStale).                          *)

Invariants ==
  /\ TypeOK
  /\ Acyclic
  /\ DisjointLeasedDirty
  /\ LeasedPathsSynced
  /\ StaleOnlyAfterExpiry
  /\ ConflictFreeWithoutExpiry

(* Structural facts needed to make the invariant inductive (checked with     *)
(* CowTreeInductive.cfg: start from every state satisfying Inv, take one     *)
(* step, and check Inv still holds). This is the "induct to time n" step:    *)
(* TLC on Spec checks reachable states; TLC on Inv as the initial predicate  *)
(* checks the induction itself, for the bounded model.                       *)
Structural ==
  /\ \A l \in Leaves : stale[l] \subseteq dirty[l]
  /\ \A l \in Leaves : ~active[l] => (dirty[l] = {} /\ stale[l] = {})
  /\ \A p \in Paths : lease[p] # None => active[lease[p]]
  /\ \A l \in Leaves : base[l] \in Nodes
  /\ tip \in Nodes
  /\ merges \subseteq Nodes \X Paths
  /\ \A l \in Leaves, p \in Paths : (active[l] /\ p \notin dirty[l]) => view[l][p] = origin[l][p]
  \* With a clean history, every dirty path is leased by its leaf. This is the conjunct
  \* the first inductiveness run found missing: a dirty, unleased path with no expiry
  \* and no rogue write is unreachable, but nothing above said so.
  /\ (~expired /\ ~rogue) => \A l \in Leaves, p \in Paths : p \in dirty[l] => lease[p] = l

Inv == Invariants /\ Structural

-----------------------------------------------------------------------------
(* Action properties *)

(* I1: snapshots never change after creation. *)
ContentImmutable ==
  [][\A n \in Nodes : content'[n] = content[n] /\ parent'[n] = parent[n]]_vars

(* I6: the tip only moves to a child of itself. History is linear per workspace. *)
TipFastForwardOnly ==
  [][tip' = tip \/ parent'[tip'] = tip]_vars

(* NoLostUpdate: a publish carries exactly the publisher's leased dirty paths; *)
(* every other path of the new tip equals the old tip.                        *)
NoLostUpdate ==
  [][tip' # tip =>
       \E l \in Leaves :
         /\ active[l]
         /\ \A p \in dirty[l] : lease[p] = l
         /\ \A p \in Paths \ dirty[l] : content'[tip'][p] = content[tip][p]]_vars

(* Merges happen only on stale paths, i.e. only as crash recovery. *)
MergesOnlyOnStale ==
  [][\A m \in merges' \ merges :
       \E l \in Leaves : active[l] /\ m[2] \in stale[l] /\ m[2] \in dirty[l]]_vars

=============================================================================
