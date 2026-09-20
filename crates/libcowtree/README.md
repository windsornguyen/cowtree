# libcowtree

Agents edit normal files in normal Git worktrees. After a native clone, the
filesystem tracks shared blocks and allocates private storage when either file
is written. Cowtree does not intercept those writes or maintain a second block
map. A saved file is working-tree state until an explicit capture records it.

`libcowtree` is the shared Rust engine boundary. Its first operation is strict
file cloning. Language interfaces must propagate failures and must never replace
an unavailable clone with an ordinary copy.

## State ownership

| State | Owner | When it changes |
|---|---|---|
| Shared file blocks | Host filesystem | Native clone and subsequent writes |
| Branches, commits, indexes | Git | Git commands |
| Working files | Editor or agent | Ordinary writes and atomic saves |
| Retained checkpoints | Managed workspace | Explicit capture and retention |
| Publication and recovery records | Rust metadata store | Validated durable transitions |

Block sharing does not provide version history, checked publication, process
isolation, or a multi-file transaction. The managed layer supplies its own
checkpoint and publication contracts. Native cloning also does not flush a
completed workspace to stable storage. The caller owns that durability boundary.

## Consolidation

The CLI and Python bindings will call the same engine. Batch operations must
own their file loops in Rust rather than crossing a language boundary per file.
Python remains an integration surface. The Rust metadata crate remains the
canonical owner of its existing durable state during the migration.

The file primitive is implemented on macOS and Linux in this layer. Windows,
batch operations, and language bindings must be qualified before replacing the
existing callers. This crate does not yet replace the Python control plane.
