# SQLite and filesystem qualification

Run the release-mode qualification example on one local filesystem that supports
copy-on-write clones. The object store, source, and leaves must be on the same
filesystem. Unsupported cloning fails; ordinary copies are never substituted.
The supplied output directory must not exist. The harness creates and preserves
its artifacts there, and never changes a supplied source checkout.

Start with a small run to validate the environment:

```sh
cargo run --release -p cowtree-metadata --example qualify -- \
  --root /path/to/new-pilot --files 16 --operations 2 --trials 1 --readers 2 \
  > pilot.json
```

The default workload uses 10,000 distinct deterministic 4 KiB files, 100 batches,
three independent fresh authorities, and two concurrent readers:

```sh
cargo run --release -p cowtree-metadata --example qualify -- \
  --root /path/to/new-qualification --seed 1 > qualification.json
```

A real-project run imports a clean Git checkout's tracked paths. For Linux, use
a case-sensitive filesystem and an existing checkout at a recorded commit:

```sh
cargo run --release -p cowtree-metadata --example qualify -- \
  --root /path/to/new-linux-qualification --source /path/to/linux \
  --operations 100 --trials 3 --seed 1 --readers 2 > linux-qualification.json
```

Bounds are 100,000 source files, 4 GiB of tracked data, 64 MiB per object, 1,000
batches per trial, ten trials, and 64 readers. Set `--readers 0`, then repeat with
2, 8, or 64 in fresh directories to measure contention. Use the same source,
seed, operation count, compiler profile, and executable for comparisons. All
trials within a run use the same seed and workload. Record dirty source changes
and the exact executable digest alongside the report when benchmarking an
uncommitted implementation; the recorded repository HEAD alone is insufficient.

## Workload and oracle

Each batch samples four distinct regular files. The harness atomically saves an
appended marker, captures their bytes through the bound-leaf API, prepares four
disjoint proposals, and publishes them together. It installs the committed
version and compares the entire manifest with an independently computed oracle.
It refreshes grants before releasing them because publication changes origins.
The source snapshot remains unchanged. This exercises file publication; it does
not establish that the modified project compiles or retains application semantics.

Each reader owns an independent Store connection and observes monotonically
increasing committed versions. One reader also measures acquisition and immediate
release of SQLite's writer lock through a separate connection. Only BUSY/LOCKED
errors permit another reader attempt. Other errors stop qualification. Readers
pause for 5 ms between samples; they are measurement load, not free observers.

After publication, the harness hashes every tracked source file and every writer
file, checks the writer's complete path set, installs a fresh receiver, and checks
its complete file paths, bytes, executable bits, and symlink targets. It then measures
maintenance. A failed check exits without emitting a successful JSON report.

## Measurements and limits

JSON includes import, clone/bind, fresh receiver installation, verification, and
maintenance time, capture/prepare/commit/install latency,
whole-workload throughput, each reader's latency and contention count, and
maintenance results. Latency histograms use 64 buckets: bucket zero is below
one microsecond; bucket `n` spans `[2^(n-1), 2^n)` microseconds, with the final
bucket saturated. Maxima and aggregate time remain exact measured durations for successful operations;
busy failures are counted separately.
The throughput median is computed from the independently reported trials.

Database and WAL lengths are observed before and after maintenance. Object
logical bytes and `st_blocks * 512` describe file accounting. **These are not
exclusive physical APFS bytes:** shared clone blocks can appear in multiple
files' allocation counts. This harness cannot establish a space-savings
percentage; use the isolated-volume space benchmark for that question.

History is retained throughout each bounded trial to keep reader versions valid;
maintenance occurs after readers stop. This run measures publication contention
and filesystem agreement, not steady-state retention bounds, concurrent garbage
collection, process-crash recovery, or power-loss durability. Run the dedicated
retention, collection-race, and fault-injection tests separately. No throughput
or recovery claim should exceed the recorded workload and failure model.
