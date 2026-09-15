----------------------- MODULE FencedPublishInduction ------------------------
EXTENDS FencedPublish
VARIABLE step
\* Enumerate every Inv-state in the finite domains, then take exactly one step.
InductiveInit == Inv /\ step = 0
OneStep == step = 0 /\ Next /\ step' = 1
=============================================================================
