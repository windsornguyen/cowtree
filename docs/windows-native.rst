Windows native clone qualification
==================================

.. note::

   Historical measurements on this page describe their recorded revisions.
   Their original harnesses remain in Git history. For the Rust-only runtime,
   use ``cargo test --workspace --all-features`` and the native benchmark in
   ``benchmarks/README.rst``. Earlier qualification is not a result for a later refactor.

The native file adapter uses ``FSCTL_DUPLICATE_EXTENTS_TO_FILE`` only when the
volume advertises ``FILE_SUPPORTS_BLOCK_REFCOUNTING``. It requires matching
volume identities, copies integrity settings, and sends cluster-aligned ranges
below 4 GiB. Failed copies remove only the destination created by that operation.
There is no ordinary-copy path.

The Windows CI jobs create disposable ReFS virtual disks on x64 Server 2025
and ARM64 Windows 11 (Dev Drive). They check empty,
sub-cluster, aligned, and partial-cluster files for exact content and independent
source/destination writes. A separate NTFS directory must reject cloning without
creating a destination. Each job detaches its own disk before deleting it.
The Dev Drive fixture reserves 64 GiB virtually, without preallocating it.

The standalone CLI uses LockFileEx for repository serialization and preserves
Git's executable-mode records without assuming POSIX permissions. Its tests run
the complete add/doctor/remove flow, dirty-removal refusal, and concurrent forks.
Managed commands fail with a typed unsupported result before creating state.
Their directory durability and supervised process ownership require a separate
Windows port before the managed lifecycle can advertise support.

References:

* `Block cloning <https://learn.microsoft.com/en-us/windows/win32/fileio/block-cloning>`_.
* `Microsoft's cluster-rounded EOF implementation
  <https://github.com/microsoft/CopyOnWrite/blob/main/lib/Windows/WindowsCopyOnWriteFilesystem.cs>`_.
