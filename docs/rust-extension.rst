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

The Python layer owns Git command construction, CLI parsing, request dataclasses,
and user-facing errors. ``cowtree.native`` contains the current syscall boundary.

Native contract
---------------

Each clone operation receives a regular source file and an absent destination.
It creates the destination exclusively, preserves its mode and modification time,
and returns only after cloning finishes. A failed clone removes only the file it
created. Cleanup failures identify the remaining path and preserve the original
failure as their cause.

The current macOS binding declares the ``clonefile(2)`` argument and return types
at its foreign function interface (FFI). Linux holds both file descriptors open
through ``FICLONE`` and metadata application. Neither path substitutes a byte copy.

A future Rust implementation must preserve that contract. Keep unsafe calls in
one FFI module, own descriptors with resource types, and retain operating-system
errors as typed sources. Keep module roots as documented exports. Unit tests stay
beside their implementation; filesystem qualification still runs on real mounts.

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
