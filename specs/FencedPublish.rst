Publication model checks
========================

``FencedPublish.tla`` models the authority required to publish disjoint deltas
into an ordered snapshot history. It complements the `workspace protocol
<../docs/workspace-protocol.rst>`_ and the existing `Workspace model <README.md>`_.
It focuses on captured proposal tokens, batches and finite induction. It omits
reservation activation and leaf drop, so it is not a refinement of Workspace.
It does not replace or reproduce the unavailable CowTree or EpochLog models.

Scope
-----

The model represents grants, revocation, observed bases, frozen proposals,
discard and atomic batch commit. Proposal origins and tokens are frozen at
submission. An unleased proposal is allowed as an input and rejected at commit.
Generations advance on every new grant, including reacquisition by the same owner.

Snapshots are finite maps from independent keys to values. History is a sequence,
so equal content can occur at different versions. The model has no pathname
hierarchy, aliases, open handles, storage failures, request deduplication, merge
drivers, cache validation, candidate-build pipeline, or implementation code.
Its atomic Commit step assumes those lower-level obligations are fulfilled.

All configurations check safety only. They explicitly disable deadlock checks
because finite round/grant budgets can exhaust progress. There is no fairness,
liveness or real-time guarantee. No state/action constraints prune successors.

Configurations
--------------

.. list-table::
   :header-rows: 1
   :widths: 36 25 39

   * - Configuration
     - Domains
     - Expected result
   * - FencedPublish.cfg
     - 2 writers, 2 paths, 2 rounds, 1 grant per path
     - Completed check of invariants and append-only history.
   * - FencedPublishExpiry.cfg
     - 2 writers, 1 path, 2 rounds, 2 grants
     - Completed check including revocation and reacquisition.
   * - FencedPublishInduction.cfg
     - 1 writer, 1 path, 2 rounds, 1 grant
     - Completed one-step induction check from every finite Inv-state.
   * - FencedPublishInductionTwoWriters.cfg
     - 2 writers, 1 path, 1 round, 1 grant
     - Completed one-step induction check from every finite Inv-state.
   * - MissingFence.cfg
     - Same domains as the expiry check
     - Exit 12 with PublishedWithAuthority violated.
   * - WholeLeaf.cfg
     - Same domains as the batch check
     - Exit 12 with UnmodifiedPathsPreserved violated.
   * - StaleBaseWitness.cfg
     - Same domains as the batch check
     - Exit 12 with NeverAcceptsStaleBase violated.

Values are ``{0, 1}`` in every configuration. The two induction configurations
use ``FencedPublishInduction.tla``. All others use ``FencedPublish.tla``.

MissingFence deliberately omits generation equality while retaining owner and
origin checks. WholeLeaf deliberately replaces the tip using a stale base tree.
These controls belong to model tests and do not describe supported operating modes.

The witness configuration negates a desired reachable behavior. It must find an
accepted proposal that changes a value while another writer's disjoint path has
changed since the proposal's base. Its expected violation is evidence of useful
progress, not a safety failure.

The induction wrapper enumerates all states satisfying ``Inv``, then permits
one step. It checks preservation only in the listed finite domains. Ordinary
reachable-state checks also cover Init. Required publication safety is included
as conjuncts of Inv. None of these checks proves an unbounded theorem.

Reproduction
------------

Use the official ``tla2tools.jar`` v1.8.0 release with SHA-256::

    20322939d1b55bb0a3f674ab34bb69b87c711a6b35559d32445cb7d7f6d3bb58

The review runs use Adoptium Java 21 on macOS arm64. Point ``TLA2TOOLS_JAR``
at the verified JAR. Copy the modules and configurations into a new task-owned
directory before running TLC so traces and state files do not modify the source.
From that directory::

    java -Xmx1024m -XX:+UseParallelGC -cp "$TLA2TOOLS_JAR" tlc2.TLC \
      -workers 2 -seed 1 -fp 0 -config FencedPublish.cfg \
      -noGenerateSpecTE -dumpTrace json counterexample.json FencedPublish.tla \
      > tlc.log 2>&1

Use the table to choose the configuration and module. Preserve the process exit
status, full log, copied inputs and their hashes. A negative test requires both
exit 12 and the named invariant violation. A parse/evaluation error or timeout
does not satisfy it. Apply a wall-time limit in the process supervisor.

The companion review packet contains completed-run receipts and concrete JSON
counterexamples. Treat a changed model, configuration or tool artifact as a new
claim requiring its own run.
