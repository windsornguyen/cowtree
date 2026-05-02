filesystem validation
=====================

``cowtree`` is useful only when the filesystem can clone file contents without
copying their data blocks. Unsupported filesystems should fail loudly.

Matrix
------

+-------------+-------------------------+---------------+
| Environment | Expected mechanism      | Status        |
+=============+=========================+===============+
| macOS APFS  | ``clonefile(2)``        | supported     |
+-------------+-------------------------+---------------+
| Linux XFS   | ``cp --reflink=always`` | planned probe |
+-------------+-------------------------+---------------+
| Linux btrfs | ``cp --reflink=always`` | planned probe |
+-------------+-------------------------+---------------+
| Linux ext4  | none                    | unsupported   |
+-------------+-------------------------+---------------+

Local Linux VM
--------------

On macOS, use the Lima/KVM VM when available:

::

    $ limactl list
    $ limactl shell kvm

Inside the VM, create separate test mounts for XFS, btrfs, and ext4, then run:

::

    $ cowtree doctor /path/to/mount
    $ uv run python benchmarks/run.py --preset quick --runs 2

The target invariant is storage, not speed: creating many worktrees should keep
new physical allocation close to metadata overhead on reflink filesystems, and
should fail on ext4 unless a real reflink-capable layer is present.
