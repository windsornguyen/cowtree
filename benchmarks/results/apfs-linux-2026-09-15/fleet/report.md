# Physical APFS worktree benchmark

1 paired trials; 4 additional worktrees; source `adc218676eef25575469234709c2d87185ca223a`.

| Stage | Git fleet GiB | cowtree fleet GiB | Fleet saving | Total saving |
|---|---:|---:|---:|---:|
| Pristine | 6.464 | 0.163 | 97.47% | 75.60% |
| 1% atomic saves | 6.466 | 0.238 | 96.32% | 74.71% |
| After churn | 6.473 | 0.223 | 96.55% | 74.90% |

![Measured physical space](space-savings.png)

Fleet savings = 1 - (cowtree stage - cowtree source) / (Git stage - Git source). Total savings subtract each empty filesystem instead. All numbers include APFS metadata and Git indexes. The source checkout and Git object database are counted once in total footprint. Later stages also include metadata written by earlier Git status checks.

Container allocation is sampled after normal detach/reattach; the sparse image's allocation is retained separately as a host high-water footprint. Operation timings exclude checkpoints and verification.

A seeded 1% sample of tracked regular C/header files receives atomic saves. Two rounds remove half the fleet (at least one tree), recreate it and append comments to another 1% sample. The second round deletes one directory externally before API cleanup. Both arms must have identical operation results and state digests.

Full manifests check source and every leaf before edits and after final churn. Intermediate checks hash every edited path plus approximately 128 clean paths in each tree. Checks cover content, modes, symlinks, HEAD, dirty paths and registry. This is sequential fault-injection testing; it does not prove crash consistency, concurrent linearizability, or successful builds of modified Linux sources.

## Operation and backing-image evidence

| Trial | Method | Create seconds | Peak image GiB | Cleanup residual MiB |
|---|---|---:|---:|---:|
| 0 | git | 104.35 | 9.456 | 16.254 |
| 0 | cowtree | 428.08 | 2.227 | 31.516 |

Images retain allocation after deletion; no image compaction was performed. Cleanup residual includes retained filesystem and Git metadata after all leaf registrations and directories have been verified absent. Timing includes sparse-image I/O and unrelated host load; it does not establish native-volume speed. Trial 0 runs Git first; additional trials alternate arm order.
