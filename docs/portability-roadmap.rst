Native filesystem portability
=============================

`Issue 12`_ covers Windows native cloning and standalone Git worktrees.
`Issue 26`_ tracks the separate managed backend: coordination, publication,
recovery, and cleanup. The current implementation has macOS and Linux managed
backends. Windows managed workspaces remain unimplemented and fail before
creating state.
``filesystems.rst`` defines the current capability probe and filesystem matrix.

Native ReFS probe
-----------------

``benchmarks/refs_probe.ps1`` ran on Windows build 26200 against a new ReFS
DevDrive in a dynamic VHDX. It verified the block-refcounting capability flag,
cloned 8 MiB through ``FSCTL_DUPLICATE_EXTENTS_TO_FILE``, compared all bytes,
and verified source-to-target and target-to-source write isolation. The probe
removed its files, and the owned VHDX was detached and deleted afterward.

Run it against an existing test ReFS volume, supplying an absent directory::

    powershell -NoProfile -File benchmarks/refs_probe.ps1 -Directory D:\cowtree-probe

That initial probe qualified the aligned native primitive. The ongoing CI suite
in ``windows/`` additionally checks partial-cluster files, standalone Git locks,
symlink kinds, executable index modes, and cleanup. See ``windows-native.rst``.
Managed publication and recovery still need the implementation below.
An NTFS system disk was left untouched; it was not reformatted for this test.

.. _Issue 12: https://github.com/windsornguyen/cowtree/issues/12
.. _Issue 26: https://github.com/windsornguyen/cowtree/issues/26

Managed implementation sequence
-------------------------------

1. **Make managed platform selection importable.** ``src/cowtree/durable.py``,
   ``workspace.py``, ``leaves.py``, ``lifecycle.py``, and ``checks.py`` import
   the Unix-only `fcntl module`_ unconditionally. Standalone native cloning
   and Git locking already select platform-specific implementations.
   Put those operations behind an explicit platform boundary so unsupported
   systems can return the documented ``cow_unavailable`` result. Test package
   imports and JSON CLI failure before any workspace, branch, or registration
   is created. Keep one selected implementation per platform; ordinary copying
   remains unsupported.

2. **Retain the native cloning contract.** ``windows.py`` implements block
   cloning behind ``native.py`` and reports it through ``fs.py`` and ``types.py``.
   It requires matching volumes and `FILE_SUPPORTS_BLOCK_REFCOUNTING`_ before
   calling `FSCTL_DUPLICATE_EXTENTS_TO_FILE`_. Verify distinct file identities, exact
   bytes and lengths, and writes in both directions. Preserve existing targets
   and remove only the destination created by this operation on failure.
   An OS version or filesystem label alone cannot satisfy the probe.

3. **Port managed lock ownership and publication.** Standalone Git locks use
   blocking ``LockFileEx`` on a dedicated file. Workspace and leaf locks still
   need shared, exclusive, blocking, and nonblocking equivalents, including
   inherited ownership across supervised processes.
   Rust ``crates/metadata/src/objects.rs`` uses ``rustix::fs::flock`` on
   an open directory to exclude garbage collection during object publication.
   Give it a stable Windows lock object with the same lifetime and exclusion
   contract. `LockFileEx`_ is a candidate to qualify on a dedicated lock file;
   its locks do not constrain mapped views. Test competing publishers,
   collection, handle closure, and process death.

   ``src/cowtree/publication.py`` currently uses ``renamex_np`` or ``renameat2``
   to publish an absent directory. Qualify an exclusive Windows rename such as
   `FILE_RENAME_INFO`_ with replacement disabled. Rust immutable objects also
   depend on exclusive hard-link publication in ``objects.rs``. Probe that
   operation on the chosen ReFS version; a successful block clone does not
   qualify every filesystem operation used by the authority.

4. **Establish durable name publication.** ``src/cowtree/durable.py`` opens
   directories with ``O_DIRECTORY``; Rust ``durability.rs`` opens and syncs them
   using Unix-compatible behavior. Implement file and name-persistence barriers
   for Windows, including the directory creation boundaries in ``database.rs``.
   `FlushFileBuffers`_ requires a handle with write access; substituting it on
   the current read-only handles is insufficient. Document and test the actual
   namespace-persistence mechanism before advertising managed workspaces.
   A successful file-data flush cannot by itself prove persistence of rename,
   link, or removal operations.

5. **Port source identity and pathname rules.** Rust ``import.rs`` uses Unix
   metadata and descriptor-relative ``openat``/``statat``/``readlinkat`` to reject
   traversed symlink parents and changed sources. The Windows implementation
   must use stable handles and reject unexpected reparse-point traversal.
   Python ``trees.py`` and ``install.py`` also require explicit treatment of
   executable modes, symlinks, hard links, case collisions, and names that
   Windows cannot represent. Test Cargo caches with real compiler metadata and
   absolute paths. Continue requiring explicit derived-cache prefixes; BSMR
   remains the action-cache authority.

.. _fcntl module: https://docs.python.org/3/library/fcntl.html
.. _FILE_SUPPORTS_BLOCK_REFCOUNTING: https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getvolumeinformationw
.. _FSCTL_DUPLICATE_EXTENTS_TO_FILE: https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_duplicate_extents_to_file
.. _LockFileEx: https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-lockfileex
.. _FILE_RENAME_INFO: https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_rename_info
.. _FlushFileBuffers: https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-flushfilebuffers

ReFS clone cases
----------------

Microsoft's `block-cloning contract`_ requires matching source/destination
volumes and integrity-stream settings, cluster-aligned ranges, destination
pre-sizing, and ranges smaller than 4 GiB. A sparse source requires a sparse
destination. The backend must discover cluster size and split large ranges.
These constraints need native tests for 4 KiB and 64 KiB clusters, zero-byte
files, sizes just below/at/above a cluster, and files above 4 GiB. Establish and
test the final partial-cluster behavior without modifying the source or
silently substituting a full copy.

Exercise integrity-stream mismatches, sparse holes, cross-volume destinations,
reparse points, already-existing targets, and injected errors between ranges.
Keep the complete clone private until all ranges and metadata are valid. Test
isolation when a clone becomes the source of another clone. Record OS build,
ReFS version, cluster size, capability flags, and CPU architecture with results.
Windows x64 and ARM64 share this contract and each need native execution before
claiming support for that architecture.

.. _block-cloning contract: https://learn.microsoft.com/en-us/windows/win32/fileio/block-cloning

Qualification and adoption
--------------------------

Use a disposable Windows host with a real ReFS volume and an unsupported volume.
After the native API cases pass, run the Rust authority tests, Python integration
tests, and mounted workspace tests there. Preserve the same independent byte
oracle across interrupted import, concurrent forks, retained checkpoints,
publication, stale-candidate rejection, corruption refusal, and collection.
Add Windows process-termination injection at the same durable boundaries as the
Unix crash suite. Verify open-handle deletion and replacement behavior explicitly.
Do not weaken source ownership or grant changes because a backend differs.

For Linux, ``scripts/fs_matrix.sh`` already provisions disposable loopback
Btrfs, XFS with and without reflinks, and ext4. Positive lanes run ``tests``,
``src``, and ``mounted``; negative lanes require refusal without leftover state.
Run it in a disposable Linux environment with loop devices and mount privileges,
and run the Rust crash suite and ``integration`` tests as well. Default container
storage and a successful container launch are insufficient filesystem evidence.
Generate any new CI lanes from ``ci/*.ts`` through Hollywood Actions.

Per-path installation continues to require quiescent builders: old descriptors
and mappings can retain the replaced files while new lookups see the replacement.
Keep process-crash, machine-crash, and storage-power-loss qualification as separate
results. Before default BSMR adoption, require production-sized Cargo cache and
large-tree import measurements on every supported deployment filesystem,
including interrupted recovery and retained-checkpoint collection.
