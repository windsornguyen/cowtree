Production readiness
====================

Cowtree is not approved for mission-critical use or default production workspace
management. Existing evidence supports supervised, opt-in developer use. This
document defines the acceptance gates for a specific deployment. A green test
suite, a fast benchmark, and an installable binary do not close these gates.

Approval scope
--------------

The deployment owner must identify Cowtree's role, its callers, and the consequence
of a wrong result. A disposable developer checkout and a tool that can cause a
wrong mission artifact to be accepted have different assurance needs. Evaluate
indirect effects through the build pipeline as well as direct operational use.

The approval must bind an exact source revision and executable hashes to an
operating system, filesystem and mount configuration, storage stack, Git version,
consumer adapter, toolchain, workload limits, and recovery policy. It must name
the authoritative source and artifact stores independently of disposable leaves.
An excluded platform need not pass qualification: Windows is a separate expansion
gate, not a prerequisite for an APFS-only deployment.

The gates below are Cowtree's proposed engineering acceptance criteria. They are
not a complete NASA compliance checklist. NASA projects determine software class,
safety relevance, applicable requirements, and required independent verification
and validation through their responsible engineering and assurance authorities.
See `NPR 7150.2D, chapter 3`_ and `NASA-STD-8739.8B`_. We have no such project
classification, approved assurance case, or mission-use authorization for Cowtree.

Gate status
-----------

``Partial`` means relevant component evidence exists, but the deployment gate is
not closed. ``Open`` means no complete acceptance evidence is recorded. Every
gate below still needs approval for a production deployment.

.. list-table:: Deployment acceptance gates
   :header-rows: 1
   :widths: 18 47 35

   * - Gate and owner
     - Evidence required to pass
     - Current evidence and gap
   * - G0: Intended use and hazards. Deployment owner and assurance reviewer.
     - Approve the input contract, admitted workloads, operating limits, and
       consequences of corrupted source, wrong cache hits, lost acknowledgements,
       and unavailable workspaces. Map each hazard to a control, observable
       acceptance test, and named owner. Identify conditions that stop a build
       or prevent an artifact from being accepted.
     - Open. The local API has a documented contract. No deployment-specific
       hazard assessment or signed acceptance decision is recorded.
   * - G1: Source and build correctness. Cowtree and consumer maintainers.
     - Check source, siblings, retained snapshots, and published candidates with
       an independent byte, mode, namespace, and symlink oracle. Require zero
       unexplained mismatches. Run real consumer builds against a clean reference
       build. Exercise every cache-key input in the approved contract, including
       dependencies, toolchain, profile, command, environment, and path assumptions.
       Nondeterministic outputs need an approved comparison rule. Test rejection
       of corrupt and stale candidates and preservation of later private edits.
     - Partial. Real Cargo reuse, independent file checks, stale candidates, and
       snapshot corruption tests pass. The actual BSMR adapter and its complete
       action-cache decisions remain unqualified. A node ID is not an action digest.
   * - G2: Process and writer ownership. Consumer adapter maintainer.
     - Demonstrate exclusive worker assignment and rejection of stale owners.
       Stop and reap every writer before capture, installation, recovery, or drop.
       Kill controllers and workers while they hold files, mappings, and locks.
       Verify that a surviving child cannot write into a reassigned or reclaimed
       leaf. Enforce the declared treatment of external editors and escaped daemons.
     - Partial. Operation locks and validation supervision have tests. Ordinary
       prepared leaves have no atomic worker-claim API. Per-path installation
       does not rebind old descriptors or provide an atomic whole-tree switch.
   * - G3: Durable acknowledgement and recovery. Storage maintainer.
     - Inject failure before and after every durable transition in import,
       capture, validation, commit, installation, and collection. Lost replies
       must resolve to the same recorded result on retry. Acknowledged data must
       remain recoverable throughout its promised retention interval. Failed or
       torn writes and flush errors must not acknowledge success. Validate
       machine-reset and storage-power-loss behavior on the deployed storage stack
       before claiming survival of those failures; a virtual process kill is
       insufficient evidence for a physical device-cache guarantee.
     - Partial. Process kills, failed barriers, interrupted installs, and lost
       acknowledgements are covered. Abrupt machine/storage-loss qualification
       remains open. Existing models assume durable atomic storage operations.
   * - G4: Retention, namespace, and resource failures. Filesystem maintainer.
     - Prove by the approved analysis and tests that collection preserves live,
       retained, pending, and recovering inputs. Exercise replacement and symlink
       races, corrupt metadata, unsupported mounts, permissions, full disks,
       exhausted descriptors, and configured admission limits. Every failure
       must preserve foreign paths and prior acknowledged state, return an
       actionable error, and allow the documented recovery procedure. Verify
       bounded storage growth and reclamation under the admitted workload.
     - Partial. Native isolation, namespace, retention, collection races, and
       several I/O failures have tests. A complete disk-full and descriptor-limit
       matrix, deployment capacity policy, and workload envelope remain open.
   * - G5: Model coverage and independent assurance. Reviewer independent of implementation.
     - Trace each safety requirement through design, code, tests, and results.
       Review the correspondence between protocol models and real operations,
       including steps abstracted as atomic. Demonstrate that deliberate broken
       controls fail. Account explicitly for executions beyond checked model
       bounds; either justify the broader argument or enforce approved bounds.
       Close safety findings through independent review and the deployment's
       required assurance process.
     - Partial. Bounded models, negative controls, and selected runtime trace
       replays exist. No checked refinement, unbounded induction, or liveness
       proof is claimed. No independent high-stakes acceptance review is recorded.
   * - G6: Production workload and operations. Service owner.
     - Predeclare fleet size, tree sizes, cache volume, churn, failure schedule,
       latency limits, recovery-time limits, and evidence retention. Run sustained
       real build/edit/fork/recovery/collection workloads at those limits on the
       intended workers. Demonstrate alerts, diagnosis, safe admission refusal,
       and operator recovery. Require zero unexplained integrity failures and
       satisfaction of the declared resource and latency budgets.
     - Open for the consumer deployment. Small functional fixtures and large Ruff
       measurements exist. A few trials do not establish tail latency, long-run
       capacity, or integrated fleet reliability. A subsecond target is separate
       from integrity approval; no arbitrary soak duration proves correctness.
   * - G7: Release, upgrade, restore, and rollback. Release owner and independent reviewer.
     - Bind CI and qualification results to the shipped artifacts. Preserve source,
       dependency, build-tool and license provenance; authenticate distributed
       artifacts. Test installation on clean target hosts, upgrades with pending
       work, and restoration of consistent database/object/Git state. Rehearse
       rollback or restoration from backup when a schema downgrade is unsupported.
       Close blocking defects, document residual risks, and approve a staged rollout
       with explicit stop conditions.
     - Partial. A relocated local macOS bundle and forward schema migrations have
       tests. The local bundle is ad-hoc signed. A qualified release, operational
       backup/restore drill, rollback rehearsal, and deployment signoff are open.

Evidence to retain
------------------

Each gate needs an owner, status, scoped acceptance criteria, commands, artifact
checksums, and an independent review result. Record the exact runtime and consumer
revisions, build configuration, host/storage configuration, test inputs, random
seeds, injected failures, observations, and unresolved deviations. Preserve failed
runs with the successful runs. Changes to a qualified dependency or environment
require an impact review and requalification of affected gates.

Keep raw logs, generated measurements, and test archives outside the source
repository in a retained artifact store. The acceptance record must reference
their immutable identities and retention policy. Do not rely on an expiring CI
log link or a developer's temporary directory as the sole release evidence.
Commit the contracts, reviewed analysis, and scripts needed to reproduce them.

Existing reproduction entry points
----------------------------------

These commands provide component evidence. They do not close the deployment
gates by themselves. Run mounted tests on an owned directory on the actual
deployment filesystem, using an absent scratch path::

    cargo test --locked --workspace --all-targets --all-features
    cargo build --locked -p cowtree-metadata
    COWTREE_EXPECT_SUPPORTED=1 PYTHONPATH=src uv run pytest src tests integration mounted \
        --basetemp /deployment-volume/owned-cowtree-tests
    uv run python scripts/check_specs.py --cache /outside-checkout/tlc-cache

The ``--basetemp`` directory belongs to pytest and may be deleted by it; never
point it at a working store or a directory containing other data. The exact
platform and artifact must match the intended deployment.

The `verification guide <verification.rst>`_ describes model boundaries and
counterexample replay. `Managed history <workspace-qualification.rst>`_ covers
seeded lifecycle histories. `Cargo qualification <cargo-workspaces.rst>`_ and
`workspace performance <performance.rst>`_ describe real workloads and their
timing boundaries. `Installation <install.rst>`_ includes the relocated-bundle
test. The `BSMR contract <bsmr-workspaces.rst>`_ defines consumer ownership.

Approval rule
-------------

Default production adoption requires all applicable gates to pass for the named
deployment and its approved use. An exception needs a documented mitigation and
explicit acceptance by the responsible engineering and assurance authorities;
an unresolved integrity failure blocks approval. Approving one build-tool role
does not approve flight software, safety control, another filesystem, or another
consumer. No benchmark, test count, or author can grant a general NASA approval.

.. _NPR 7150.2D, chapter 3: https://nodis3.gsfc.nasa.gov/displayDir.cfm?Internal_ID=N_PR_7150_002D_&page_name=Chapter3
.. _NASA-STD-8739.8B: https://standards.nasa.gov/standard/nasa/nasa-std-87398
