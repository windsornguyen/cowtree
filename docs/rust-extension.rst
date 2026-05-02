rust extension plan
===================

Keep the first release pure Python. That makes the CLI, API, errors, and
benchmark contract easy to inspect.

Add Rust only after benchmarks show the Python per-file loop is the bottleneck.
The candidate hot path is narrow:

* walk tracked Git paths
* create parent directories
* clone regular files with ``clonefile(2)`` or Linux reflinks
* recreate symlinks
* return a typed summary to Python

The Python layer should continue owning Git command construction, CLI parsing,
Pydantic models, and user-facing errors.

Packaging
---------

Use PyO3 with maturin if the native extension lands. PyO3 is the Rust binding
layer for native Python modules, and maturin is the packaging tool that builds
those modules into Python wheels.

Keep the extension as a library, not a second CLI binary. Maturin documents that
shipping both a binary and library can duplicate wheel size; a Python entrypoint
calling one native module avoids that trap.

Binary size budget
------------------

The native module should stay boring:

* no async runtime unless benchmarks prove it is needed
* no Git implementation in Rust
* no alternate fallback path
* no broad dependency graph

If the wheel grows materially, measure it in CI and document the tradeoff.
