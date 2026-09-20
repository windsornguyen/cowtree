Grouped durability
==================

Contract
--------

macOS permits file writeout followed by a shared device-cache flush. Apple's
`fsync manual <https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fsync.2.html>`_
distinguishes host-to-drive writeout from the later cache flush. Its
`fcntl manual <https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fcntl.2.html>`_
specifies that ``F_FULLFSYNC`` performs file synchronization and asks the drive to
persist all buffered data. ``fsync`` alone is insufficient on macOS.

This is also an established Git technique:
`core.fsyncMethod=batch <https://github.com/git/git/blob/master/Documentation/config/core.adoc#L651-L666>`_
writes out each object before one final device-cache flush. Git documents this
mode as expected to preserve its fsync safety on APFS and HFS+. Its
`implementation <https://github.com/git/git/blob/v2.45.0/bulk-checkin.c#L85-L115>`_
keeps objects private until that barrier completes. Cowtree likewise keeps ready
records and import progress uncommitted until the entire group is flushed.

The kernel still delegates filesystem behavior: Apple's
`XNU dispatch <https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/kern_descrip.c#L3664-L3680>`_
passes ``F_FULLFSYNC`` through the filesystem's ioctl operation. Public XNU source
is not an APFS implementation proof. The ordering below follows the documented
interface and the Git precedent; successful calls and process-crash recovery are
separate evidence from sudden power-loss recovery.

Ordering
--------

A group contains only quiescent files and directories on one filesystem:

1. Complete each file's content and metadata updates, then call POSIX ``fsync``
   on that file. Check every error. A macOS Rust ``File::sync_all`` is unsuitable
   for this step because `Rust's Apple implementation
   <https://github.com/rust-lang/rust/blob/1.97.0/library/std/src/sys/fs/unix.rs#L1303-L1313>`_
   already issues ``F_FULLFSYNC``; use the explicit ``rustix::fs::fsync`` wrapper.
2. Complete name creation, linking, renaming, and temporary-name removal. Call
   ``fsync`` on every changed directory, from children to parents, including
   the parent of each newly created tree. File writeout does not replace
   directory writeout.
3. Call ``F_FULLFSYNC`` on the group's root directory. A successful return is
   the device-cache boundary for the preceding completed writeouts.
4. Only then commit SQLite progress or write the caller's ready record.
   That record retains its own existing persistence protocol.

No caller may acknowledge one member of a group before step 3. On any writeout
or final-barrier failure, leave progress and ready records unadvanced. Retry must
verify existing immutable bytes and finish their flush before acknowledging them.
The filesystem adapter checks every open descriptor's device identity. Its
existing operation locks and immutable captures supply quiescence.

The Rust object store applies grouping only inside ``put_batch`` while the
metadata writer transaction excludes collection. Ordinary object publication
retains its existing flush sequence. Python applies grouping to complete node
captures and leaf initialization; ``sync_file`` and ``write_record`` retain
existing behavior.

Linux remains different: the
`Linux fsync contract <https://man7.org/linux/man-pages/man2/fsync.2.html>`_
already includes device-cache handling and separately requires directory fsync.
The Linux implementation retains each normal fsync. The
`kernel cache-control documentation <https://kernel.org/doc/html/latest/block/writeback_cache_control.html>`_
describes flush and Force Unit Access ordering; it does not authorize substituting
macOS's writeout-only assumption on Linux.

Probe
-----

Run the checked-in macOS probe against an existing directory and a quiescent
source tree containing at least 64 regular files::

    RUSTC_WRAPPER='' cargo run --offline -p cowtree-metadata --example group_durability -- /existing/output /frozen/source

The probe sorts source paths, reads the first 64 payloads before timing, and
copies them into a fresh flat publication directory. Every strategy receives
identical bytes and name operations. Three rotated rounds compare legacy double
full-sync, one full-sync per file, and file writeout with one group full-sync.
JSON records include the source digest, bytes, flush counts, timing, and verified
file count. Setup, source reads, verification, and cleanup are outside the timer.

On this APFS device, 64 frozen Ruff files containing 333,082 bytes produced medians
of 496.995 ms, 272.959 ms, and 46.089 ms respectively. All 576 file comparisons
passed across nine samples. Grouping was 5.92 times faster than one full-sync per
file in this probe. This is an isolated flush comparison; it is not a complete
workspace import benchmark or a promise of subsecond large-tree initialization.

Qualification
-------------

Rust tests interrupt import before and after the group barrier and every import
transaction boundary. A failed group flush returns a typed error without advancing
the cursor. Python tests check writeout order, one final macOS flush, error
propagation, and process death around tree flushes and ready records. Existing
source and sibling bytes remain independently checked during recovery.

Apple's current
`disk-write guidance <https://developer.apple.com/documentation/xcode/reducing-disk-writes>`_
still describes ``F_FULLFSYNC`` as best effort. These tests qualify process
crashes and requested operating-system ordering. They do not prove device
firmware behavior, sudden power loss, or arbitrary multi-device storage stacks.
