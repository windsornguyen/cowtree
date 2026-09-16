# Metadata qualification results

All four measured trials passed: **80 atomic batches, 104,646 file checks,
10,991 concurrent reader snapshots, and no BUSY failures**. Each batch changed
four files. Publication throughput varied substantially on the shared host;
these measurements do not establish a speedup over the earlier short baseline.

The current qualification measures local SQLite publication with real file
capture and installation on APFS. It uses one writer, two reader connections,
four file edits per atomic batch, and a complete filesystem oracle. Every
successful trial checks the unchanged source, the writer, and a freshly installed
receiver, including bytes, file paths, executable bits, and symlink targets.

## Recorded build

Host: Apple M5 Max, 48 GiB RAM, macOS 26.5.2, APFS. Other work was running
on the host; these are observed workload results rather than isolated hardware
limits.

The measurement executable was compiled in release mode from a frozen copy of
the Rust workspace. Its SHA-256 is
`782fa7dd454e6648112d44579df176d79baedfb52c2cfc246001a762ed6c92d2`.
The captured source files were unchanged during capture and compilation. The
working repository was still under development, so its HEAD alone does not
identify this measurement build.

The synthetic run uses 10,000 distinct 4 KiB files, 20 batches per trial, three
fresh authorities, and seed 1. Each trial repeats the same sampled edits. The
public-project run uses CPython 3.13.0 at
`60403a5409ff2c3f3b07dd2ca91a7a3e096839c7`, 20 batches, one trial, and seed 1.
The source checkout is clean, separate from earlier benchmark fixtures, and
unchanged by the workload. Commands and measurement definitions are in
[the qualification guide](qualification.md).

## Observations

Each trial contains 20 batches and two concurrent readers. Commit means exclude
capture, preparation, and installation; batch throughput includes the complete
publication loop and its per-batch manifest oracle. The final full receiver
installation is measured separately.

| Source / trial | Files | Batches/s | Mean commit (ms) | Initial import (s) | Fresh receiver install (s) |
| --- | ---: | ---: | ---: | ---: | ---: |
| Synthetic / 1 | 10,000 | 1.058 | 579.0 | 107.7 | 165.5 |
| Synthetic / 2 | 10,000 | 1.230 | 498.3 | 100.3 | 245.6 |
| Synthetic / 3 | 10,000 | 0.642 | 923.5 | 113.2 | 258.5 |
| CPython 3.13.0 / 1 | 4,882 | 0.945 | 571.3 | 67.8 | 162.0 |

The synthetic median was **1.058 batches/s**. The CPython result is one trial;
there is no repeated-trial estimate for that project. Bulk durable import and
fresh installation account for most of the 22.3-minute qualification duration.
The existing-file capture/publication loop is a separate, shorter measurement.

Maximum observed writer-lock acquisition time was 561.1 ms across the synthetic
trials and 122.5 ms for CPython. These samples include contention from every
operation and host scheduling; moving commit verification outside the writer
transaction does not eliminate other lock holders or guarantee a latency bound.

After maintenance, each synthetic authority occupied 331,776 database bytes and
zero WAL bytes; CPython occupied 307,200 database bytes and zero WAL bytes.
Epochs and receipts were deliberately retained for the entire bounded trial, so
these figures are not evidence of steady-state retention behavior. Every trial
ended at version 21, with all four members of each batch sharing one version.

The disk guard observed at least **150.73 GiB free** throughout execution.
Preserved artifacts account for 1.05 GiB by per-file allocation; that accounting
includes shared blocks and is not an exclusive APFS physical-space measurement.
The captured sources, executable digest, and clean CPython source were unchanged
after the runs. Raw JSON includes all trials and latency histograms.

## Interpretation

The earlier 10,000-file run contained only two batches. It measured a mean
commit latency of 671.8 ms while full payload verification held SQLite's writer
transaction. This is a short diagnostic baseline, not a duration-matched control
for the longer qualification. Different trial lengths and a shared, non-isolated
host prevent a statistical throughput-improvement claim. Filesystem caches are
not cleared between trials.

A deterministic concurrency test separately establishes the locking change:
an unrelated metadata writer previously timed out with `SQLITE_BUSY` while
verification was paused. It now progresses during that pause. Publication still
checks the exact candidate, membership, attempt, readiness, tip, lease tokens,
and origins in its final transaction. Revocation, supersession, tip advancement,
and collection after abort cannot acknowledge a stale candidate. Existing
committed receipts remain available even after their snapshots are collected.

These workloads qualify the recorded operations and filesystem state. They do
not prove power-loss durability, arbitrary process-crash recovery, application
build correctness after sampled edits, or exclusive physical storage savings.
Object `st_blocks` values include shared clone blocks and must not be interpreted
as unique physical disk usage. The dedicated crash and isolated-volume space
benchmarks cover separate questions.
