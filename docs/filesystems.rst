Filesystem validation
=====================

``cowtree`` requires a successful native copy-on-write clone on the source and
target filesystem. An ordinary full copy is never an accepted substitute.
``cowtree doctor PATH`` probes this requirement; filesystem names alone do not
establish support.

Required matrix
---------------

+----------------------+-------------------------+------------------+
| Environment          | Mechanism               | Expected result  |
+======================+=========================+==================+
| macOS APFS           | ``clonefile(2)``         | add succeeds     |
+----------------------+-------------------------+------------------+
| Linux btrfs          | ``FICLONE``              | add succeeds     |
+----------------------+-------------------------+------------------+
| Linux XFS, reflink=1  | ``FICLONE``              | add succeeds     |
+----------------------+-------------------------+------------------+
| Linux XFS, reflink=0  | unsupported             | add refuses      |
+----------------------+-------------------------+------------------+
| Linux ext4           | unsupported             | add refuses      |
+----------------------+-------------------------+------------------+

Integration suite
-----------------

Run from the repository root on a filesystem that supports cloning::

    COWTREE_EXPECT_SUPPORTED=1 uv run --group test pytest -q

``COWTREE_EXPECT_SUPPORTED=1`` makes missing native clone support a test failure.
Without that setting, tests requiring native CoW skip when it is unavailable.
Use ``--basetemp /mount/owned-test-directory`` to put every pytest repository on a
specific mount. Pytest deletes that directory, so reserve it for this run.

The integration suite checks byte and pathname preservation, executable modes,
symlinks, source and clone write isolation, clones used as sources, explicit
source paths with a different current directory, source eligibility, rollback,
protected removal, and concurrent add/list/remove from threads and processes.
The concurrent lanes use 16 threads and 8 processes. Set
``COWTREE_STRESS_WORKERS`` to an integer from 2 to 64 to change both counts.
The bounded large fixture contains 256 small files and an 8 MiB file. Raw
non-UTF-8 filename cases skip only when the filesystem rejects creating the
fixture; carriage returns, line feeds and tabs are tested independently.

Failure-injection tests replace the clone operation with a local I/O error or a
source mutation. Successful checkout tests use the real native clone operation.
These tests establish behavior and isolation; they do not measure physical
allocation or claim a performance qualification.

Linux loop-device matrix
------------------------

``scripts/fs_matrix.sh`` creates fresh btrfs, XFS with reflink enabled, XFS with
reflink disabled, and ext4 filesystems. Run it inside a disposable Linux VM with
loop devices and mount privileges. Install ``uv``, Git, util-linux, btrfs-progs,
xfsprogs, and e2fsprogs first. From a checkout of this repository::

    sudo env "PATH=$PATH" bash scripts/fs_matrix.sh

To place its owned temporary files on a particular existing directory::

    sudo env "PATH=$PATH" COWTREE_MATRIX_ROOT=/mnt/scratch bash scripts/fs_matrix.sh

Each filesystem uses a 768 MiB sparse image. The script runs the complete suite
on positive filesystems and requires both a negative doctor result and an add
failure without leftover branch, directory or registration on negative
filesystems. Unexpected support or missing expected support fails the matrix.
No lane is silently skipped.

The script owns one temporary directory, one active mount and one loop device
at a time. Exit and signal traps unmount and detach only those resources. A
cleanup failure retains its owned directory and reports its path. It never
formats an existing disk or searches for unrelated mounts to remove. The uv
cache and environment also live in its temporary directory.

A green APFS run does not establish Linux support. Record the operating system,
filesystem configuration, revision and test result for each environment that
actually ran. Allocation measurements require a separate workload and physical
extent accounting appropriate to that filesystem.
