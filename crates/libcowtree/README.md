# libcowtree

The native engine used by the native CLI and managed workspace library. The filesystem
owns block sharing during edits. This library owns admission, traversal,
cloning, metadata preservation, and cleanup of the state it creates.

## Operations

- `add_worktree` locks the repository, pins a source commit, reserves an absent
  destination, registers it with Git, and verifies the result. Checkout mode
  clones existing tracked files. Committed mode asks Git to populate the final
  destination and applies the same ownership and cleanup rules.
- `list_worktrees` and `remove_worktree` use the same repository lock. Removing
  a worktree preserves its branch.
- `clone_file` requires a regular source and an absent target. No byte-copy
  substitute is permitted.
- `scan_tree`, `clone_tree`, and `populate_tree` implement snapshot path policy
  and content or metadata capture. These currently require Unix.
- `inspect_path` tests actual clone support and independent writes.

`AddRequest.submodules` selects rejection, empty uninitialized children (the
default), or independent pinned child repositories. `submodules.rs` plans the
children from immutable gitlinks. `submodule_objects.rs` CoW-clones local object
data into fresh metadata. `submodule_record.rs` owns per-worktree policy and
activation records. Managed read-only dependency capture remains separate.

See [standalone submodules](../../docs/standalone-submodules.rst) for the Git
worktree configuration change, refusal cases, and removal contract.

An add owns only its new directories, registration, and newly created branch.
Rollback retires the registration before deleting that branch with an expected
commit ID. If another writer moves the branch, rollback preserves it and reports
the unresolved cleanup. Cancellation uses the same path.

## Ownership

| State | Owner |
| --- | --- |
| Shared blocks and live-write isolation | Filesystem |
| Refs, indexes, and worktree registrations | Git |
| Standalone transaction and tree operations | This library |
| Managed snapshot and publication records | `cowtree-metadata` |
| Managed filesystem installation and recovery | `cowtree-workspace` |

`creation.rs` defines one transaction for both source modes. Git execution is
shared through [`crates/git`](../git/README.md). `platform/` contains clone
primitives. `tree_scan.rs` and `tree_clone.rs` implement snapshot capture and
materialization. `creation_tests.rs` injects failures only in test builds.

The repository's [native engine guide](../../docs/rust-extension.rst) covers
packaging, benchmarks, and qualification limits. Live file edits are not
automatic checkpoints, and standalone creation does not recover from SIGKILL
or host power loss.
