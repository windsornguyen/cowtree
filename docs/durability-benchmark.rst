Durability call benchmark
=========================

Run this probe before changing the filesystem flush boundary. It compares the
legacy double flush, ``std::fs::File::sync_all``, and the current single full
flush, using fresh 4 KiB files, immutable publication, and a directory barrier.
The ``single_full`` strategy calls the production durability module.

Reproduction
------------

Choose an existing directory on the filesystem under test. Keep other import,
build, and benchmark processes idle during measurement. Run from the repository
root::

    git rev-parse HEAD
    git diff -- crates/metadata/src/durability.rs
    rustc -Vv
    df -h .
    RUSTC_WRAPPER='' cargo build --locked --release --example durability
    target/release/examples/durability . 64 > durability.jsonl

The count is the publications per sample, bounded to 1 through 4096. The probe
runs three rounds, rotating strategy order. Each sample uses a new temporary
directory and deletes it after verifying every published file against the input bytes.
Any filesystem or verification error exits nonzero; retain only complete runs
with nine successful samples. No existing caller files are removed.

``publication_ms`` includes temporary creation, writing, file flush, hard-link
publication, removal of the temporary name, and directory flush. The individual
file and directory flush times are subsets of that total. Readback, fixture
directory creation, and final cleanup are outside the measured interval. The
barrier counts are logical calls made by the probe, not kernel trace counts.
Report each sample and the median of each strategy's three samples; do not gate
CI on wall-clock thresholds from a developer machine.

Source check
------------

Both `Rust 1.85.0`_ (the minimum supported version) and `Rust 1.97.0`_ implement
``sync_all`` on Apple targets using ``fcntl(F_FULLFSYNC)``. The installed Rust
1.97.1 source has the same implementation. At Cowtree commit
``1163e2b039bfe7f6fb8ad231212ee8e63abfbe6b``, ``sync_file`` first calls
``sync_all`` and then ``rustix::fs::fcntl_fullfsync`` on macOS. Directory barriers
use this same helper. Thus a successful publication normally requests four
full syncs through legacy Cowtree and two through std. These counts are derived
from source; interrupted calls can be retried by the implementation. On Linux the
three strategies call the same std sync operation.

The current macOS implementation calls ``rustix::fs::fcntl_fullfsync`` directly
once for each file or directory barrier. It preserves the explicit required OS
operation and propagates errors. It avoids both the duplicate request and a
dependency on the standard library's future choice of persistence primitive.
Other platforms continue to use ``sync_all``. A macOS unit test checks that an
unsupported full flush on ``/dev/null`` returns an error.

.. _Rust 1.85.0: https://github.com/rust-lang/rust/blob/1.85.0/library/std/src/sys/pal/unix/fs.rs#L1124-L1134
.. _Rust 1.97.0: https://github.com/rust-lang/rust/blob/1.97.0/library/std/src/sys/fs/unix.rs#L1303-L1313

APFS measurement
----------------

Measured on September 19, 2026, against the unchanged durability module at the
commit above: macOS 26.5.2 (25F84), Rust 1.97.1
(``8bab26f4f68e0e26f0bb7960be334d5b520ea452``), ``aarch64-apple-darwin``, APFS
Data volume ``/dev/disk3s5``. The volume had approximately 197 GiB available.
The release build used ``RUSTC_WRAPPER=''`` and an isolated Cargo target directory.
Each row contains 64 file barriers, 64 directory barriers, and 64 successful
file comparisons. Times are milliseconds.

========= ====== =========== ========== ===============
Strategy  Repeat Publication File sync  Directory sync
========= ====== =========== ========== ===============
Cowtree   1      774.24      376.96     343.47
std       1      518.66      238.22     239.36
std       2      521.41      237.74     241.78
Cowtree   2      723.18      343.71     343.35
Cowtree   3      624.59      306.86     280.84
std       3      539.30      257.71     239.26
========= ====== =========== ========== ===============

Median publication time was 723.18 ms through Cowtree and 521.41 ms through std,
a 27.9% reduction (1.39x throughput). Median file flush time was 343.71 versus
238.22 ms; median directory flush time was 343.35 versus 239.36 ms. All 384
published files matched. This was the baseline before changing the macOS helper;
the single-full-flush comparison below measures the resulting path. It does not
estimate the speedup of the reported 25.6-minute Ruff import: hashing, SQLite commits, and Python
orchestration are outside this probe.

Single full flush
-----------------

The same device and toolchain ran all three strategies in rotating order after
the change. An earlier run overlapped a Cargo fixture test and was excluded.
All other task-owned disk benchmarks were idle for these nine samples:

============= ====== =========== ========== ===============
Strategy      Repeat Publication File sync  Directory sync
============= ====== =========== ========== ===============
legacy_double 1      788.34      368.67     382.49
std           1      549.29      259.32     241.44
single_full   1      531.12      252.38     237.61
std           2      597.11      270.34     277.43
single_full   2      657.07      289.79     273.61
legacy_double 2      654.31      321.80     286.22
single_full   3      542.76      261.74     241.16
legacy_double 3      668.58      321.20     306.15
std           3      532.71      249.69     237.59
============= ====== =========== ========== ===============

Median publication time was 668.58 ms for the legacy double flush, 549.29 ms for
std, and 542.76 ms for the explicit single full flush: an 18.8% reduction from
legacy Cowtree (1.23x throughput). All 576 published files matched. The single
full flush and std results were close; the choice of an explicit full flush
preserves Cowtree's required macOS operation. The observed timing variation
rules out treating either run's percentage as a universal speedup.

Run the Rust publication, collection, concurrency, and crash tests after any
change to this boundary::

    RUSTC_WRAPPER='' cargo test --locked --workspace --all-targets --all-features

Qualification boundary
----------------------

The byte checks prove successful publication and immediate readback. This probe
does not stop processes, reboot the host, cut storage power, or measure a full
SQLite import. A faster result alone cannot establish recovery correctness or
power-loss durability. Qualify any runtime change separately with the existing
publication and collection crash matrix and an independent byte oracle.

Both strategies retain the existing per-path installation contract. Syncing a
replacement inode does not rebind an already open descriptor or memory mapping;
build processes must be quiescent across installation. Atomic directory-wide
visibility requires a separate workspace boundary.
