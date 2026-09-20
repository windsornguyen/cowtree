Windows native clone qualification
==================================

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

This layer qualifies the native file primitive, not the complete Windows CLI
or managed workspace lifecycle. Repository locking, executable-mode semantics,
durable directory operations, and supervised process ownership need separate
Windows contracts and tests before those interfaces can advertise support.

References:

* `Block cloning <https://learn.microsoft.com/en-us/windows/win32/fileio/block-cloning>`_.
* `Microsoft's cluster-rounded EOF implementation
  <https://github.com/microsoft/CopyOnWrite/blob/main/lib/Windows/WindowsCopyOnWriteFilesystem.cs>`_.
