# Validation supervision

The native CLI starts this supervisor before a validation command. Stdin carries
an inherited check-lock descriptor. The supervisor retains it, passes a duplicate
to the child, gives the child a null stdin, and starts a private process group. The supervisor also has its own group, so a
terminal interrupt to the coordinator cannot remove the deadline.
A deadline, log limit, or leader exit triggers group cleanup. The leader remains
unreaped until the kill request, preventing its numeric group ID from being reused
for another process. Only then is its exit acknowledged and its lock released.

```text
coordinator -> native supervisor [check lock] -> validation process group
```

`run(argv, timeout_seconds)` returns the child's code, 124 for a deadline, or 125
for the 128 MiB log limit. Typed errors retain process and cleanup failures.

```sh
cargo test -p cowtree-cli --all-features validation_lock_survives
```

The integration test kills the coordinator, checks that collection preserves the
running validation leaf, and confirms retirement after the native deadline.
