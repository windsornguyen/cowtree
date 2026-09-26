Native profiling
================

The Rust CLI continues to call Git. Native code reduces frontend and traversal
overhead, while Git remains responsible for indexes, refs, checkout conversions,
and worktree registration. Complete creation still includes a content check
against a fresh destination index.

The profiling baseline was ``codex/native-engine`` at
``682948b44efcc4214b852c90465b8e909efb2637``. Measurements below were taken on
2026-09-20. Raw traces, binaries, source snapshots, and samples were retained
outside the checkout. They are local qualification, not hosted CI or a release.

Syscalls and ownership
----------------------

LLDB on macOS stopped at ``fclonefileat`` during a real add. Its stack passed
through native cloning, tracked-file population, and worktree creation. Linux
``strace -f -c`` on a real reflink-XFS mount in the existing development VM
showed duplicate source inspection and destination existence probes.

The implementation reuses population's source metadata and lets the native
primitive enforce exclusive destination creation. It still opens the source
without following its final symlink and validates the opened descriptor.

For 512 tracked files, including Git children and the clone probe, ``statx``
calls fell from 2,072 to 1,047. Total traced calls fell from 11,964 to 10,938.
Tracing changes timing, so these counts establish removed operations. Five
untraced Linux trials measured medians of 71.6 and 70.1 ms with overlapping
ranges. That small latency difference is not a demonstrated speed advantage.

Parallel population
-------------------

An isolated APFS probe compared the existing native clone primitive with 1, 4,
16, and 64 workers. Three rotating trials on 8,192 files measured clone-phase
medians of 999, 445, 1,141, and 1,500 ms. Four workers reduced this phase by 55%.
More workers increased contention.

The integrated path uses at most four workers, including the calling thread.
Each file retains its own source descriptor and exclusive destination creation.
Scoped threads finish before success or rollback. A controlled failure test
holds one real clone in flight and verifies that an error cannot return early.
Cancellation uses one shared atomic request. Recheck worker count when changing
filesystem backends or workload shape.

Complete creation
-----------------

Five rotating trials compared identical release builds with one or four clone
workers and installed Git 2.54.0. Each fixture has 8 KiB files. Startup, admission,
cloning, index creation, and verification are timed. Git status and cleanup are
checked outside the timer. Medians on this ARM64 Mac and APFS were:

.. list-table:: Complete creation, milliseconds
   :header-rows: 1

   * - Files
     - One worker
     - Four workers
     - Git
   * - 512
     - 154.7
     - 129.3
     - 43.3
   * - 8,192
     - 1,519.6
     - 1,095.2
     - 617.4

The larger four-worker range was 1,046.0-1,132.0 ms. The one-worker range was
1,422.9-1,587.2 ms. These are synthetic, compressible files on a shared host. The recorded fixtures
repeat a 64-byte digest per file. The native example below generates its own
compressible fixture, so compare methods within each run.
The reduction is about 16% and 28% for these fixtures. Git parity remains open.

The destination content check must remain. A native regression test changes
source bytes while preserving the source index's trusted metadata. Ordinary
source Git status reports clean, but creation rejects the copied bytes and
removes its owned destination.

Git call audit
--------------

A subsequent call audit reduced ordinary detached creation from 15 top-level
Git invocations to 13. The source snapshot already resolves the default HEAD.
The existing worktree record supplies the destination HEAD, so a separate
``rev-parse`` is unnecessary. Source HEAD is still checked after cloning, and
the destination's fresh-index content verification remains.

Managed projections no longer initialize an already empty private index.
Verification of a reused commit reads its tree and ordered parents in one Git
call. A complete validation fixture fell from 45 calls to 42. Recovery's ref
existence checks remain: Git rejects deletion of a missing ref when supplied
with an expected object ID.

Nine rotating trials of these query reductions measured 512-file creation at
161.1 ms before and 148.7 ms after. The paired median change was -12.5 ms, with
a bootstrap 95% interval of -24.2 to -4.6 ms. At 8,192 files the interval crossed
zero, so that fixture did not establish a latency improvement. Git's complete
destination content check remains the main measured cost.

The subsequent simplification removes that temporary checkout. Committed mode
uses Git's exclusive checkout directly in the final destination, runs the hook
there, and verifies against a fresh index. Filesystem admission and rollback
ownership remain explicit. Hook and filter regression tests retain the content
checks needed after externally configured Git operations.

Nine rotating APFS trials of committed creation measured 265.4 ms before and
181.9 ms after at 512 files, and 2,541.1 ms before and 1,292.4 ms after at 8,192
files. Direct Git measured 67.9 ms and 807.4 ms on the same fixtures. These are
31.5% and 49.1% reductions in median latency, not Git parity. Each file held
8 KiB of a repeated byte. Validation and cleanup remained outside the timer.

Committed creation now issues 16 top-level Git calls instead of 19. Managed
checkpointing falls from 23 to 15 calls, capture from 35 to 27, and validation
from 42 to 33 in the traced lifecycle. Capture holds one session rather than
reopening five. Shared command setup, persisted repository context, and one
checkpoint publication owner remove repeated work without adding a dispatcher.

Reproduce
---------

::

    cargo build --release --locked -p cowtree-cli
    cargo run --release -p cowtree-cli --example benchmark -- \
        --binary target/release/cowtree --files 8192 --trials 5 \
        --output /tmp/cowtree-benchmark.json

On Linux, use a real reflink-capable filesystem::

    strace -f -c -o /tmp/cowtree-syscalls.txt \
        /absolute/cowtree add --detach /same-filesystem/new-worktree

On macOS, LLDB can inspect the syscall boundary::

    lldb -- /absolute/cowtree add --detach /same-filesystem/new-worktree
    breakpoint set -n fclonefileat -i 1
    run
    thread backtrace
    breakpoint disable 1
    continue

The ignore count skips the capability probe before the tracked-file clone.
Remove only the created test worktree afterward. Debugger and syscall traces
establish code paths and operations, not uninstrumented latency.
