Git projection
==============

``GitProjection`` writes selected source paths to the existing repository's
object database using a temporary index. It does not change the caller's HEAD,
branch, or index. The candidate tree supplies its attributes. Derived and
ephemeral entries are excluded by the caller's explicit source manifest.

Private node refs live under ``refs/cowtree/nodes/``. Workspace tip refs live
under ``refs/cowtree/tips/``. Ref updates require the expected previous commit,
and an exact retry is idempotent. The projection cannot update ordinary branches.
The workspace journal must persist the projected commit before publishing its
ref so recovery can retry the same value after an uncertain result.

The repository's Git identity and filters apply. A filter or missing identity
can fail projection explicitly. This implementation indexes all selected paths;
it does not claim changed-only complexity or bypass Git's conversion rules.
An explicit reuse request requires the exact source tree and parent list of
the validated commit. It fails on a mismatch instead of creating another commit.
The authoritative filesystem snapshot still records working-file bytes.

Run ``cargo test -p cowtree-cli --all-features --tests`` to check source selection, normal
Git attribute conversion, index isolation, ref restrictions, and retries.
Gitlink inspection
------------------

``GitRepository.snapshot`` rejects submodules by default. Explicitly selecting
``SubmodulePolicy.MATERIALIZE_PINNED`` returns their paths and commit identities
in ``Checkout.submodules`` alongside ordinary ``Checkout.files``. This records
the parent tree without flattening or dropping its gitlinks. It does not fetch,
initialize, or verify child checkouts. Managed admission owns those checks.
