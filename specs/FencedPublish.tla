---------------------------- MODULE FencedPublish ----------------------------
EXTENDS Naturals, Sequences, FiniteSets, TLC

\* Publication-boundary model. Paths are independent keys, not POSIX names.
CONSTANTS Writers, Paths, Values, None, Initial,
          MaxRounds, MaxFence, EnforceFences, DeltaOnly
ASSUME /\ Writers # {} /\ Paths # {} /\ Values # {}
       /\ None \notin Writers /\ Initial \in Values
       /\ MaxRounds \in Nat /\ MaxFence \in Nat

Snapshots == [Paths -> Values]
InitialTree == [p \in Paths |-> Initial]
Proposals == [paths : SUBSET Paths, values : Snapshots, origins : Snapshots,
              tokens : [Paths -> 0..MaxFence], base : 0..MaxRounds]
EmptyProposal == [paths |-> {}, values |-> InitialTree, origins |-> InitialTree,
                  tokens |-> [p \in Paths |-> 0], base |-> 0]

VARIABLES history, owner, generation, token, origin, base, proposal,
          lastAuthorized, lastPreserved, staleBaseAccepted
vars == <<history, owner, generation, token, origin, base, proposal,
          lastAuthorized, lastPreserved, staleBaseAccepted>>

Round == Len(history) - 1
Tip == history[Len(history)]
Holds(w, p) == owner[p] = w /\ token[w][p] = generation[p]
Overlay(tree, delta) ==
    [p \in Paths |-> IF p \in delta.paths THEN delta.values[p] ELSE tree[p]]
Writes(batch) == UNION {proposal[w].paths : w \in batch}
UsesStaleBase(batch) ==
    \E w \in batch :
        /\ proposal[w].base < Round
        /\ \E p \in proposal[w].paths : proposal[w].values[p] # Tip[p]
        /\ \E p \in Paths \ Writes(batch) :
             owner[p] \in Writers \ batch /\ history[proposal[w].base + 1][p] # Tip[p]
Disjoint(batch) ==
    \A a, b \in batch : a # b => proposal[a].paths \cap proposal[b].paths = {}
Authorized(batch) ==
    \A w \in batch : \A p \in proposal[w].paths :
        /\ owner[p] = w
        /\ proposal[w].tokens[p] = generation[p]
        /\ proposal[w].origins[p] = Tip[p]
CanCommit(batch) ==
    \A w \in batch : \A p \in proposal[w].paths :
        /\ owner[p] = w
        /\ proposal[w].origins[p] = Tip[p]
        /\ (~EnforceFences \/ proposal[w].tokens[p] = generation[p])
Candidate(batch) ==
    IF DeltaOnly
    THEN [p \in Paths |->
          IF p \in Writes(batch)
          THEN proposal[CHOOSE w \in batch : p \in proposal[w].paths].values[p]
          ELSE Tip[p]]
    ELSE LET w == CHOOSE a \in batch : TRUE
         IN Overlay(history[proposal[w].base + 1], proposal[w])

TypeOK ==
    /\ history \in UNION {[1..n -> Snapshots] : n \in 1..(MaxRounds + 1)}
    /\ history[1] = InitialTree
    /\ owner \in [Paths -> Writers \cup {None}]
    /\ generation \in [Paths -> 0..MaxFence]
    /\ token \in [Writers -> [Paths -> 0..MaxFence]]
    /\ origin \in [Writers -> Snapshots]
    /\ base \in [Writers -> 0..MaxRounds]
    /\ proposal \in [Writers -> Proposals]
    /\ lastAuthorized \in BOOLEAN /\ lastPreserved \in BOOLEAN
    /\ staleBaseAccepted \in BOOLEAN
IssuedTokens == \A w \in Writers : \A p \in Paths : token[w][p] <= generation[p]
ExistingBases ==
    \A w \in Writers : base[w] <= Round /\ proposal[w].base <= Round
SyncedOrigins == \A w \in Writers : \A p \in Paths : Holds(w, p) => origin[w][p] = Tip[p]
PublishedWithAuthority == lastAuthorized
UnmodifiedPathsPreserved == lastPreserved
Inv == TypeOK /\ IssuedTokens /\ ExistingBases /\ SyncedOrigins
       /\ PublishedWithAuthority /\ UnmodifiedPathsPreserved
NeverAcceptsStaleBase == ~staleBaseAccepted

Init ==
    /\ history = <<InitialTree>>
    /\ owner = [p \in Paths |-> None]
    /\ generation = [p \in Paths |-> 0]
    /\ token = [w \in Writers |-> [p \in Paths |-> 0]]
    /\ origin = [w \in Writers |-> InitialTree]
    /\ base = [w \in Writers |-> 0]
    /\ proposal = [w \in Writers |-> EmptyProposal]
    /\ lastAuthorized = TRUE /\ lastPreserved = TRUE
    /\ staleBaseAccepted = FALSE

Grant(w, p) ==
    /\ owner[p] = None /\ generation[p] < MaxFence
    /\ owner' = [owner EXCEPT ![p] = w]
    /\ generation' = [generation EXCEPT ![p] = @ + 1]
    /\ token' = [token EXCEPT ![w][p] = generation[p] + 1]
    /\ origin' = [origin EXCEPT ![w][p] = Tip[p]]
    /\ UNCHANGED <<history, base, proposal, lastAuthorized, lastPreserved, staleBaseAccepted>>

\* Revoke covers release/expiry. Old proposals and tokens remain observable.
Revoke(p) ==
    /\ owner[p] # None
    /\ owner' = [owner EXCEPT ![p] = None]
    /\ UNCHANGED <<history, generation, token, origin, base, proposal,
                   lastAuthorized, lastPreserved, staleBaseAccepted>>

ReadBase(w) ==
    /\ base[w] # Round
    /\ base' = [base EXCEPT ![w] = Round]
    /\ UNCHANGED <<history, owner, generation, token, origin, proposal,
                   lastAuthorized, lastPreserved, staleBaseAccepted>>

\* Unleased proposals are allowed. Safety is enforced at publication.
Propose(w, paths, values) ==
    /\ paths # {} /\ proposal[w].paths = {}
    /\ proposal' = [proposal EXCEPT ![w] =
        [paths |-> paths,
         values |-> [p \in Paths |-> IF p \in paths THEN values[p] ELSE Initial],
         origins |-> [p \in Paths |-> IF p \in paths THEN origin[w][p] ELSE Initial],
         tokens |-> [p \in Paths |-> IF p \in paths THEN token[w][p] ELSE 0],
         base |-> base[w]]]
    /\ UNCHANGED <<history, owner, generation, token, origin, base,
                   lastAuthorized, lastPreserved, staleBaseAccepted>>

Discard(w) ==
    /\ proposal[w].paths # {}
    /\ proposal' = [proposal EXCEPT ![w] = EmptyProposal]
    /\ UNCHANGED <<history, owner, generation, token, origin, base,
                   lastAuthorized, lastPreserved, staleBaseAccepted>>

Commit(batch) ==
    /\ batch # {} /\ Round < MaxRounds
    /\ \A w \in batch : proposal[w].paths # {}
    /\ Disjoint(batch)
    /\ DeltaOnly \/ Cardinality(batch) = 1
    /\ CanCommit(batch)
    /\ LET next == Candidate(batch)
       IN /\ history' = Append(history, next)
          /\ lastAuthorized' = Authorized(batch)
          /\ lastPreserved' = \A p \in Paths \ Writes(batch) : next[p] = Tip[p]
          /\ origin' = [w \in Writers |-> [p \in Paths |->
               IF w \in batch /\ p \in proposal[w].paths THEN next[p] ELSE origin[w][p]]]
    /\ staleBaseAccepted' = (staleBaseAccepted \/ UsesStaleBase(batch))
    /\ proposal' = [w \in Writers |-> IF w \in batch THEN EmptyProposal ELSE proposal[w]]
    /\ UNCHANGED <<owner, generation, token, base>>

Next ==
    \/ \E w \in Writers, p \in Paths : Grant(w, p)
    \/ \E p \in Paths : Revoke(p)
    \/ \E w \in Writers : ReadBase(w) \/ Discard(w)
    \/ \E w \in Writers, paths \in SUBSET Paths, values \in Snapshots :
           Propose(w, paths, values)
    \/ \E batch \in SUBSET Writers : Commit(batch)

Spec == Init /\ [][Next]_vars
LogAppendOnly ==
    [][Len(history') \in {Len(history), Len(history) + 1}
       /\ SubSeq(history', 1, Len(history)) = history]_vars
=============================================================================
