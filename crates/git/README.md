# Git execution

Both Cowtree workflows use this synchronous client to run Git. Git owns refs,
indexes, checkout conversions, hooks, and worktree registration.

```text
standalone transaction --+
                        +--> Client --> git
managed workspace ------+
```

`Client` owns the selected command directory and inherited lock descriptors.
`locked_at` uses an already known common directory. Managed workspaces retain
that directory in their durable config instead of rediscovering it per command.
Domain code owns admission, publication, and rollback decisions.

| Surface | Contract |
| --- | --- |
| `discover`, `at`, `select` | Select command context and preserve held locks. |
| `command`, `output`, `capture`, `text`, `input` | Pass argument arrays and preserve Git failures. |
| `directory`, `lock`, `locked_at` | Acquire process-owned or inherited locks. |
| `head`, `status` | Read Git state with explicit status options. |

There is no async runtime or shell dispatch. Input is written while output is
drained so a full pipe cannot deadlock. Managed Unix operations explicitly pass
their locks to Git children. Standalone callers retain process-owned locks until
Git returns, keeping their existing crash-recovery boundary.

```sh
cargo test -p cowtree-git
```

See [Git worktrees](https://git-scm.com/docs/git-worktree) and the neighboring
[standalone](../libcowtree/README.md) and [managed](../workspace/README.md) contracts.
