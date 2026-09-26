Benchmarking Cowtree
====================

Measure the operation the caller pays for. Complete creation includes executable
startup, Git admission, registration, cloning, and content verification::

    cargo build --release --locked -p cowtree-cli
    cargo run --release -p cowtree-cli --example benchmark -- \
        --binary target/release/cowtree --files 8192 --trials 5 \
        --output /tmp/cowtree-benchmark.json

The native harness rotates Git and Cowtree, checks every destination's bytes and
Git status outside the timer, and retires only its owned worktrees. Failed
fixtures remain available for inspection. Payloads are deterministic and highly
compressible. They do not represent every source repository.

Use ``--files 512`` to expose startup overhead and a larger file count to expose
traversal and clone costs. ``--bytes-per-file`` changes payload size. Report raw
samples and medians. Do not compare runs taken under different host load as a
controlled speedup. See `native profiling <../docs/native-profiling.rst>`_.

Physical APFS allocation
------------------------

The historical allocation experiment at ``f97d7cc`` used separate APFS images,
settled filesystem allocation counters, complete inventories, and controlled
edit/delete/recreate histories. Its harness is retained at that immutable
revision in Git history. See `space efficiency <../docs/space-efficiency.rst>`_
for results and measurement boundaries.

Logical file size and ``du`` do not measure shared physical blocks accurately.
The creation benchmark does not report allocation savings. Warm-cache reuse,
initial import, fresh managed forks, and prepared leaves are separate workloads.
Keep their historical receipts tied to the implementation that produced them.
