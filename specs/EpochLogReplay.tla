---------------------------- MODULE EpochLogReplay ----------------------------
EXTENDS EpochLog
CONSTANT Scenario
VARIABLE step

Writer == CHOOSE leaf \in Leaves : TRUE
Key == CHOOSE path \in Paths : TRUE
Changed == CHOOSE value \in Values : value # v0
ReplayInit == Init /\ step = 0
ReplayNext ==
  /\ CASE step = 0 -> Fork(Writer, 1)
       [] step = 1 -> Acquire(Writer, Key)
       [] step = 2 -> Write(Writer, Key, Changed)
       [] step = 3 -> Propose(Writer)
       [] step = 4 -> Sync(Writer)
       [] step = 5 -> Write(Writer, Key, v0)
       [] step = 6 -> IF Scenario = "discard" THEN Expire(Key)
                     ELSE Commit(Pending(Writer))
       [] step = 7 -> IF Scenario = "discard" THEN Discard(Writer, Key)
                     ELSE Sync(Writer)
       [] step = 8 -> Sync(Writer)
  /\ step' = step + 1
ReplaySpec == ReplayInit /\ [][ReplayNext]_<<vars, step>>
ReplayIncomplete == step < 9
=============================================================================
