# cowtree

`cowtree` creates normal Git worktrees, then populates tracked files with
filesystem copy-on-write clones.

The goal is space efficiency. A machine running many agent branches should not
materialize the same checked-out file payload dozens of times.

## Interface

```bash
cowtree add <git-worktree-add-options> <path> [commit-ish]
cowtree list [--json]
cowtree remove [--force] <path>
cowtree doctor [path]
```

## Guarantees

- Runtime dependencies: none.
- Python support: 3.10+.
- Unsupported filesystems fail loudly.
- Shell commands are represented as `list[str]`, not shell strings.
- Inline tests are stripped from built distributions.

## Non-goals

`cowtree` is not a branch dashboard, TUI, process manager, or agent
orchestrator. Tools with richer workflows should call the small CLI/API.
