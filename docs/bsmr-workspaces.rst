BSMR workspace boundary
=======================

Cowtree materializes isolated working directories. BSMR remains the authority for
action identities, cache validity, hit/miss decisions, output ownership and eviction.
A Cowtree node ID identifies workspace history; it must never stand in for a
BSMR action digest.

Consumer sequence
-----------------

1. Select a retained source checkpoint and explicit derived/ephemeral prefixes.
   Source includes lockfiles. Credentials, sockets and runtime state are ephemeral.
2. Fork a private leaf. Eligible cache bytes are inherited hints. BSMR validates
   their toolchain, target, profile, environment, command and dependency inputs
   before accepting a cache hit.
3. Run one writer per leaf. Finish and reap its processes before capture, seal,
   sync, resolution or drop. Cowtree's lock does not fence arbitrary external
   editors, retained file descriptors or memory mappings.
4. Return build outputs to BSMR through its existing action-cache interface.
   Derived directories are excluded from Cowtree source publication. If source
   changes should be shared, capture, prepare, check and commit that exact candidate.
5. Drop the leaf through Cowtree and collect unreferenced nodes. BSMR separately
   manages its authoritative action-cache entries.

Never share a writable ``CARGO_TARGET_DIR`` across independent leaves or choose
a silent ordinary-copy backend on an unsupported filesystem. When Cargo creates
hard-linked outputs, ``derived_hardlinks=DerivedHardlinks.CLONE`` is an explicit
opt-in to independent per-path cache files; source hard links remain unsupported.
Use the default rejection policy when cache correctness depends on alias identity.

Qualification gate
------------------

The local core suite covers independent byte/mode checks, private checkpoints,
checked publication, stale candidates, process kills, collection and resumed
large import. The portable `Cargo workload <cargo-workspaces.rst>`_ checks real
compiler freshness, build-script environment changes, derived hard links and
source/sibling isolation. The `import benchmark <initial-import.rst>`_ records
protocol work and elapsed time.

Default adoption requires the `production gates <production-readiness.rst>`_
for the exact worker operating system, filesystem, storage configuration, and
production build inputs. Qualify each additional deployment platform separately.
Windows support is not a prerequisite for an APFS-only deployment. A Windows
deployment does require the complete `platform adapter <portability-roadmap.rst>`_;
a ReFS clone probe does not qualify locking, publication, or recovery.

There is no BSMR action-cache implementation in Cowtree, and no default BSMR
cutover in this change. Its public Python/JSON lifecycle is the materialization
contract the adapter should consume.
