----------------------------- MODULE Workspace -----------------------------
EXTENDS Naturals, Sequences, FiniteSets, TLC

CONSTANTS Leaves, Paths, Values, InitialValue, NoLeaf,
          MaxVersion, MaxGeneration, FenceChecks

VARIABLES epochs, active, owner, generation, token, installed,
          pinned, origin, view, dirty, base, acceptedAuthority,
          staleBaseCommitted, staleTokenRejected

vars == <<epochs, active, owner, generation, token, installed,
          pinned, origin, view, dirty, base, acceptedAuthority,
          staleBaseCommitted, staleTokenRejected>>

Tip == Len(epochs) - 1
Snapshot == epochs[Len(epochs)]
Current(leaf, path) == /\ owner[path] = leaf
                       /\ token[leaf][path] = generation[path]
DirtyPaths(leaf) == {path \in Paths : dirty[leaf][path]}
Authorized(leaf) == \A path \in DirtyPaths(leaf) :
                     /\ Current(leaf, path)
                     /\ installed[leaf][path]

Init ==
    /\ epochs = <<[path \in Paths |-> InitialValue]>>
    /\ active = [leaf \in Leaves |-> TRUE]
    /\ owner = [path \in Paths |-> NoLeaf]
    /\ generation = [path \in Paths |-> 0]
    /\ token = [leaf \in Leaves |-> [path \in Paths |-> 0]]
    /\ installed = [leaf \in Leaves |-> [path \in Paths |-> FALSE]]
    /\ origin = [leaf \in Leaves |-> Snapshot]
    /\ pinned = origin
    /\ view = origin
    /\ dirty = [leaf \in Leaves |-> [path \in Paths |-> FALSE]]
    /\ base = [leaf \in Leaves |-> 0]
    /\ acceptedAuthority = TRUE
    /\ staleBaseCommitted = FALSE
    /\ staleTokenRejected = FALSE

\* Reservation and installation are separate interleavable operations.
Reserve(leaf, path) ==
    /\ active[leaf]
    /\ ~dirty[leaf][path]
    /\ owner[path] = NoLeaf
    /\ generation[path] < MaxGeneration
    /\ owner' = [owner EXCEPT ![path] = leaf]
    /\ generation' = [generation EXCEPT ![path] = @ + 1]
    /\ token' = [token EXCEPT ![leaf][path] = generation[path] + 1]
    /\ installed' = [installed EXCEPT ![leaf][path] = FALSE]
    /\ pinned' = [pinned EXCEPT ![leaf][path] = Snapshot[path]]
    /\ base' = [base EXCEPT ![leaf] = Tip]
    /\ UNCHANGED <<epochs, active, origin, view, dirty, acceptedAuthority,
                    staleBaseCommitted, staleTokenRejected>>

Activate(leaf, path) ==
    /\ active[leaf]
    /\ Current(leaf, path)
    /\ ~installed[leaf][path]
    /\ installed' = [installed EXCEPT ![leaf][path] = TRUE]
    /\ origin' = [origin EXCEPT ![leaf][path] = pinned[leaf][path]]
    /\ view' = [view EXCEPT ![leaf][path] = pinned[leaf][path]]
    /\ UNCHANGED <<epochs, active, owner, generation, token, pinned,
                    dirty, base, acceptedAuthority,
                    staleBaseCommitted, staleTokenRejected>>

Edit(leaf, path, value) ==
    /\ active[leaf]
    /\ Current(leaf, path)
    /\ installed[leaf][path]
    /\ value # view[leaf][path]
    /\ view' = [view EXCEPT ![leaf][path] = value]
    /\ dirty' = [dirty EXCEPT ![leaf][path] = value # origin[leaf][path]]
    /\ UNCHANGED <<epochs, active, owner, generation, token, installed,
                    pinned, origin, base, acceptedAuthority,
                    staleBaseCommitted, staleTokenRejected>>

\* One proposal per leaf; its entire dirty set commits in one transaction.
\* FenceChecks = FALSE deliberately removes the publication authority gate.
Publish(leaf) ==
    /\ active[leaf]
    /\ DirtyPaths(leaf) # {}
    /\ Tip < MaxVersion
    /\ (FenceChecks => Authorized(leaf))
    /\ epochs' = Append(epochs, [path \in Paths |->
          IF dirty[leaf][path] THEN view[leaf][path] ELSE Snapshot[path]])
    /\ origin' = [origin EXCEPT ![leaf] =
          [path \in Paths |-> IF dirty[leaf][path]
                             THEN view[leaf][path] ELSE origin[leaf][path]]]
    /\ dirty' = [dirty EXCEPT ![leaf] = [path \in Paths |-> FALSE]]
    /\ base' = [base EXCEPT ![leaf] = Tip + 1]
    /\ acceptedAuthority' = Authorized(leaf)
    /\ staleBaseCommitted' = (base[leaf] < Tip)
    /\ UNCHANGED <<active, owner, generation, token, installed, pinned, view,
                    staleTokenRejected>>

RejectStale(leaf) ==
    /\ active[leaf]
    /\ DirtyPaths(leaf) # {}
    /\ ~Authorized(leaf)
    /\ staleTokenRejected' = TRUE
    /\ UNCHANGED <<epochs, active, owner, generation, token, installed,
                    pinned, origin, view, dirty, base, acceptedAuthority,
                    staleBaseCommitted>>

\* Revocation keeps the old leaf's edits, but invalidates its token.
Revoke(path) ==
    /\ owner[path] # NoLeaf
    /\ generation[path] < MaxGeneration
    /\ owner' = [owner EXCEPT ![path] = NoLeaf]
    /\ generation' = [generation EXCEPT ![path] = @ + 1]
    /\ UNCHANGED <<epochs, active, token, installed, pinned, origin, view, dirty,
                    base, acceptedAuthority,
                    staleBaseCommitted, staleTokenRejected>>

\* Release returns authority while retaining local edits for explicit recovery.
Release(leaf, path) ==
    /\ active[leaf]
    /\ Current(leaf, path)
    /\ Revoke(path)

DiscardStale(leaf, path) ==
    /\ active[leaf]
    /\ ~Current(leaf, path)
    /\ token[leaf][path] # 0
    /\ token' = [token EXCEPT ![leaf][path] = 0]
    /\ installed' = [installed EXCEPT ![leaf][path] = FALSE]
    /\ view' = [view EXCEPT ![leaf][path] = origin[leaf][path]]
    /\ dirty' = [dirty EXCEPT ![leaf][path] = FALSE]
    /\ UNCHANGED <<epochs, active, owner, generation, pinned, origin, base,
                    acceptedAuthority, staleBaseCommitted, staleTokenRejected>>

Drop(leaf) ==
    /\ active[leaf]
    /\ \A path \in Paths : owner[path] = leaf => generation[path] < MaxGeneration
    /\ active' = [active EXCEPT ![leaf] = FALSE]
    /\ owner' = [path \in Paths |-> IF owner[path] = leaf THEN NoLeaf ELSE owner[path]]
    /\ generation' = [path \in Paths |->
          IF owner[path] = leaf THEN generation[path] + 1 ELSE generation[path]]
    /\ token' = [token EXCEPT ![leaf] = [path \in Paths |-> 0]]
    /\ installed' = [installed EXCEPT ![leaf] = [path \in Paths |-> FALSE]]
    /\ dirty' = [dirty EXCEPT ![leaf] = [path \in Paths |-> FALSE]]
    /\ UNCHANGED <<epochs, pinned, origin, view, base, acceptedAuthority,
                    staleBaseCommitted, staleTokenRejected>>

Next == \/ \E leaf \in Leaves, path \in Paths :
               \/ Reserve(leaf, path) \/ Activate(leaf, path)
               \/ Release(leaf, path) \/ DiscardStale(leaf, path)
               \/ \E value \in Values : Edit(leaf, path, value)
        \/ \E leaf \in Leaves : Publish(leaf) \/ RejectStale(leaf) \/ Drop(leaf)
        \/ \E path \in Paths : Revoke(path)
        \/ UNCHANGED vars

Spec == Init /\ [][Next]_vars

TypeOK ==
    /\ epochs \in Seq([Paths -> Values])
    /\ Len(epochs) \in 1..(MaxVersion + 1)
    /\ active \in [Leaves -> BOOLEAN]
    /\ owner \in [Paths -> Leaves \cup {NoLeaf}]
    /\ generation \in [Paths -> 0..MaxGeneration]
    /\ token \in [Leaves -> [Paths -> 0..MaxGeneration]]
    /\ installed \in [Leaves -> [Paths -> BOOLEAN]]
    /\ pinned \in [Leaves -> [Paths -> Values]]
    /\ origin \in [Leaves -> [Paths -> Values]]
    /\ view \in [Leaves -> [Paths -> Values]]
    /\ dirty \in [Leaves -> [Paths -> BOOLEAN]]
    /\ base \in [Leaves -> 0..Tip]
    /\ acceptedAuthority \in BOOLEAN
    /\ staleBaseCommitted \in BOOLEAN
    /\ staleTokenRejected \in BOOLEAN

OwnerActive == \A path \in Paths : owner[path] # NoLeaf => active[owner[path]]
Exclusive == \A path \in Paths, a, b \in Leaves :
                 (Current(a, path) /\ Current(b, path)) => a = b
CurrentOrigin == \A leaf \in Leaves, path \in Paths :
                    (active[leaf] /\ installed[leaf][path] /\ Current(leaf, path)) =>
                    origin[leaf][path] = Snapshot[path]
CleanView == \A leaf \in Leaves, path \in Paths :
                (active[leaf] /\ ~dirty[leaf][path]) =>
                view[leaf][path] = origin[leaf][path]
DirtyScope == \A leaf \in Leaves, path \in Paths :
                 (active[leaf] /\ dirty[leaf][path]) =>
                 /\ installed[leaf][path]
                 /\ token[leaf][path] > 0
                 /\ (Current(leaf, path) \/ token[leaf][path] < generation[path])
AcceptedAuthority == acceptedAuthority

HistoryAppendOnly == [][ /\ Len(epochs') >= Len(epochs)
                         /\ \A i \in DOMAIN epochs : epochs'[i] = epochs[i] ]_vars
MonotoneFences == [][\A path \in Paths : generation'[path] >= generation[path]]_vars

\* These deliberately false invariants produce reachability witness traces.
NoStaleBaseCommit == ~staleBaseCommitted
NoStaleTokenRejection == ~staleTokenRejected
=============================================================================
