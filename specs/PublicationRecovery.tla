------------------------- MODULE PublicationRecovery -------------------------
EXTENDS Naturals, Sequences, FiniteSets, TLC

\* Process-crash abstraction of durable candidate preparation and publication.
\* Keys are independent exclusive writers; each may have one pending request.
\* Candidate IDs name immutable (parent, key, token, bytes) records, not content alone.
CONSTANTS Keys, MaxCandidates, MaxToken, BindValidation, ProtectPublished
ASSUME /\ Keys # {} /\ MaxCandidates \in Nat /\ MaxCandidates > 0
       /\ MaxToken \in Nat /\ MaxToken > 0

Ids == 1..MaxCandidates
Roots == 0..MaxCandidates
FirstKey == CHOOSE key \in Keys : TRUE

VARIABLES alive, used, pending, built, objects, ready, validated, validationParent,
          parent, key, capturedToken, generation, held, history, acked, bindings,
          authorized, clientPins
vars == <<alive, used, pending, built, objects, ready, validated, validationParent,
          parent, key, capturedToken, generation, held, history, acked, bindings,
          authorized, clientPins>>
Tip == history[Len(history)]
Published == {history[index] : index \in 1..Len(history)}
Parents == {parent[id] : id \in pending}
LiveRoots == {0} \cup Published \cup Parents \cup (ready \cap pending) \cup clientPins
\* Preparation pins its candidate even before that object's durable write finishes.
Protected == {0} \cup Parents \cup pending \cup clientPins
             \cup IF ProtectPublished THEN Published ELSE Published \ acked

TypeOK ==
  /\ alive \in BOOLEAN
  /\ used \subseteq Ids /\ pending \subseteq used /\ built \subseteq pending
  /\ objects \subseteq Roots /\ ready \subseteq used /\ validated \subseteq used
  /\ parent \in [Ids -> Roots] /\ key \in [Ids -> Keys]
  /\ capturedToken \in [Ids -> 0..MaxToken]
  /\ validationParent \in [Ids -> Roots]
  /\ generation \in [Keys -> 0..MaxToken] /\ held \in [Keys -> BOOLEAN]
  /\ history \in Seq(Roots) /\ Len(history) \in 1..(MaxCandidates + 1)
  /\ history[1] = 0 /\ Cardinality(Published) = Len(history)
  /\ acked \subseteq Ids /\ bindings \subseteq Ids \X Ids
  /\ authorized \in BOOLEAN /\ clientPins \subseteq Roots
OnePending == \A first, second \in pending : key[first] = key[second] => first = second
NoDanglingRef == LiveRoots \subseteq objects
AckedRecoverable == acked \subseteq Published \cap objects
ValidateBoundToId ==
  \A pair \in bindings : pair[1] = pair[2] /\ validationParent[pair[2]] = parent[pair[1]]
PublishedWithAuthority == authorized
Invariants == TypeOK /\ OnePending /\ NoDanglingRef /\ AckedRecoverable
              /\ ValidateBoundToId /\ PublishedWithAuthority

Init ==
  /\ alive = TRUE
  /\ used = {} /\ pending = {} /\ built = {} /\ objects = {0}
  /\ ready = {} /\ validated = {} /\ acked = {} /\ bindings = {}
  /\ parent = [id \in Ids |-> 0] /\ key = [id \in Ids |-> FirstKey]
  /\ capturedToken = [id \in Ids |-> 0]
  /\ validationParent = [id \in Ids |-> 0]
  /\ generation = [path \in Keys |-> 0] /\ held = [path \in Keys |-> FALSE]
  /\ history = <<0>> /\ authorized = TRUE /\ clientPins = {}

Grant(path) ==
  /\ alive /\ ~held[path] /\ generation[path] < MaxToken
  /\ generation' = [generation EXCEPT ![path] = @ + 1]
  /\ held' = [held EXCEPT ![path] = TRUE]
  /\ UNCHANGED <<alive, used, pending, built, objects, ready, validated, validationParent,
                  parent, key, capturedToken, history, acked, bindings, authorized, clientPins>>
Revoke(path) ==
  /\ alive /\ held[path]
  /\ held' = [held EXCEPT ![path] = FALSE]
  /\ UNCHANGED <<alive, used, pending, built, objects, ready, validated, validationParent,
                  parent, key, capturedToken, generation, history, acked, bindings, authorized, clientPins>>

Prepare(id, path) ==
  /\ alive /\ id \notin used /\ held[path]
  /\ \A request \in pending : key[request] # path
  /\ used' = used \cup {id} /\ pending' = pending \cup {id}
  /\ parent' = [parent EXCEPT ![id] = Tip]
  /\ key' = [key EXCEPT ![id] = path]
  /\ capturedToken' = [capturedToken EXCEPT ![id] = generation[path]]
  /\ UNCHANGED <<alive, built, objects, ready, validated, validationParent,
                  generation, held, history, acked, bindings, authorized, clientPins>>
Build(id) ==
  /\ alive /\ id \in pending \ objects /\ id \notin built
  /\ built' = built \cup {id}
  /\ UNCHANGED <<alive, used, pending, objects, ready, validated, validationParent,
                  parent, key, capturedToken, generation, held, history, acked, bindings, authorized, clientPins>>
Persist(id) ==
  /\ alive /\ id \in built
  /\ objects' = objects \cup {id} /\ built' = built \ {id}
  /\ UNCHANGED <<alive, used, pending, ready, validated, validationParent,
                  parent, key, capturedToken, generation, held, history, acked, bindings, authorized, clientPins>>
MarkReady(id) ==
  /\ alive /\ id \in pending \cap objects /\ id \notin ready
  /\ ready' = ready \cup {id}
  /\ UNCHANGED <<alive, used, pending, built, objects, validated, validationParent,
                  parent, key, capturedToken, generation, held, history, acked, bindings, authorized, clientPins>>
Validate(id) ==
  /\ alive /\ id \in pending \cap ready /\ id \notin validated
  /\ validated' = validated \cup {id}
  /\ validationParent' = [validationParent EXCEPT ![id] = parent[id]]
  /\ UNCHANGED <<alive, used, pending, built, objects, ready,
                  parent, key, capturedToken, generation, held, history, acked, bindings, authorized, clientPins>>
Commit(id, validation) ==
  /\ alive /\ id \in pending \cap ready /\ validation \in validated
  /\ ~BindValidation \/ (validation = id /\ validationParent[validation] = parent[id])
  /\ parent[id] = Tip
  /\ held[key[id]] /\ capturedToken[id] = generation[key[id]]
  \* SQLite commits the new tip and durable request receipt in one transaction.
  /\ history' = Append(history, id)
  /\ pending' = pending \ {id}
  /\ bindings' = bindings \cup {<<id, validation>>}
  /\ authorized' = held[key[id]] /\ capturedToken[id] = generation[key[id]]
  /\ UNCHANGED <<alive, used, built, objects, ready, validated, validationParent,
                  parent, key, capturedToken, generation, held, acked, clientPins>>
Acknowledge(id) ==
  /\ alive /\ id \in Published \ acked
  /\ acked' = acked \cup {id}
  /\ UNCHANGED <<alive, used, pending, built, objects, ready, validated, validationParent,
                  parent, key, capturedToken, generation, held, history, bindings, authorized, clientPins>>
Abort(id) ==
  /\ alive /\ id \in pending
  /\ pending' = pending \ {id} /\ built' = built \ {id}
  /\ UNCHANGED <<alive, used, objects, ready, validated, validationParent,
                  parent, key, capturedToken, generation, held, history, acked, bindings, authorized, clientPins>>
Pin(id) ==
  /\ alive /\ id \in objects \ clientPins
  /\ clientPins' = clientPins \cup {id}
  /\ UNCHANGED <<alive, used, pending, built, objects, ready, validated, validationParent,
                  parent, key, capturedToken, generation, held, history, acked, bindings, authorized>>
Unpin(id) ==
  /\ alive /\ id \in clientPins
  /\ clientPins' = clientPins \ {id}
  /\ UNCHANGED <<alive, used, pending, built, objects, ready, validated, validationParent,
                  parent, key, capturedToken, generation, held, history, acked, bindings, authorized>>
GC(id) ==
  /\ alive /\ id \in objects \ Protected
  /\ objects' = objects \ {id}
  /\ UNCHANGED <<alive, used, pending, built, ready, validated, validationParent,
                  parent, key, capturedToken, generation, held, history, acked, bindings, authorized, clientPins>>
Crash ==
  /\ alive /\ alive' = FALSE /\ built' = {}
  /\ UNCHANGED <<used, pending, objects, ready, validated, validationParent,
                  parent, key, capturedToken, generation, held, history, acked, bindings, authorized, clientPins>>
Restart ==
  /\ ~alive /\ alive' = TRUE
  /\ UNCHANGED <<used, pending, built, objects, ready, validated, validationParent,
                  parent, key, capturedToken, generation, held, history, acked, bindings, authorized, clientPins>>

Next ==
  \/ \E path \in Keys : Grant(path) \/ Revoke(path)
  \/ \E id \in Ids, path \in Keys : Prepare(id, path)
  \/ \E id \in Ids : Build(id) \/ Persist(id) \/ MarkReady(id) \/ Validate(id)
                         \/ Acknowledge(id) \/ Abort(id)
  \/ \E id, validation \in Ids : Commit(id, validation)
  \/ \E id \in Roots : Pin(id) \/ Unpin(id) \/ GC(id)
  \/ Crash \/ Restart
Spec == Init /\ [][Next]_vars
LogAppendOnly == [][ /\ Len(history') \in {Len(history), Len(history) + 1}
                    /\ \A index \in 1..Len(history) : history'[index] = history[index] ]_vars
=============================================================================
