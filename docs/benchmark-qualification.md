# APFS repeatability qualification

This run broadens the earlier single-pair Linux measurement with three paired
trials on the complete CPython 3.13.0 source tree. It exercises native CoW Git
worktree creation and churn. The SQLite managed-workspace installation path is a
separate implementation and is not measured by this harness.

## Workload

- Source: official `python/cpython`, tag `v3.13.0`, commit
  `60403a5409ff2c3f3b07dd2ca91a7a3e096839c7`.
- Complete tracked manifest: 4,882 files, 106,139,186 bytes; no sparse checkout or
  subtree selection. Of those files, 1,005 regular C/header files are eligible for edits.
- Four additional detached worktrees per arm; seeds 314159, 314160, and 314161.
- Trial order: Git then cowtree, cowtree then Git, Git then cowtree.
- Each round selects 11 C/header files per leaf (44 per full fleet). The first
  round atomically rewrites selected files with appended C comments. Two later
  rounds remove half the fleet, recreate those leaves, and append comments in place.
- The final churn round externally deletes one worktree before verifying its
  prunable registration and explicitly removing it through the corresponding API.
- Each arm uses an independent case-sensitive APFS sparse image with 2 GiB capacity.
  Every space sample follows normal detach/reattach and requires three equal
  allocation-counter readings. The harness reserves 80 GiB of free host space.

The source and every leaf are fully hashed before edits and after final churn.
Intermediate checks hash all edited paths plus a fixed clean sample. The oracle
checks bytes, modes, symlinks, Git HEAD, dirty paths, filesystem paths, and worktree
registrations. All worktree operations complete before the next operation starts.

## Results

Completed on 2026-09-15 local time on an Apple M5 Max with 48 GiB RAM, macOS
26.5.2, Python 3.10.20, and Apple Git 2.50.1. The run completed in approximately
15 minutes. All six images were detached, used 2.57 GiB of host allocation in total,
and left 154 GiB free on the host. No image compaction was performed.

**This workload also does not establish 99% savings.** Median additional-fleet
savings were 97.95% pristine, 97.72% after atomic saves, and 97.54% after churn.

| Stage | Git fleet bytes, median | cowtree fleet bytes, median | Fleet savings, median (range) | Total savings, median |
| --- | ---: | ---: | ---: | ---: |
| Pristine | 484,007,936 | 9,945,088 | 97.95% (97.93–97.95) | 74.54% |
| 1% atomic saves | 484,069,376 | 11,055,104 | 97.72% (97.61–97.75) | 74.37% |
| After churn | 484,646,912 | 11,902,976 | 97.54% (97.54–97.55) | 74.26% |

Every pair matched all 23 recorded stages. Timestamp-normalized operation histories
were identical within each pair: 158 operations per arm, 948 operations overall.
All 54 verification checkpoints passed, hashing 348,040 file instances and
7,538,384,704 bytes across the six arms. All allocation samples settled. Final
cleanup verified that every leaf directory and Git registration was gone.

Creation timings were noisy: Git took 4.00–63.85 seconds per four-leaf fleet
(median 6.35 seconds); cowtree took 10.13–71.56 seconds (median 22.24 seconds).
These include sparse-image I/O and unrelated host load and do not establish native
volume speed. The isolated allocation counters support the space comparison.

### Per-trial receipts

| Trial / seed | Stage | Git additional bytes | cowtree additional bytes | Savings |
| --- | --- | ---: | ---: | ---: |
| 0 / 314159 | Pristine | 484,044,800 | 10,018,816 | 97.9302% |
| 0 / 314159 | 1% atomic saves | 484,093,952 | 11,571,200 | 97.6097% |
| 0 / 314159 | After churn | 484,646,912 | 11,939,840 | 97.5364% |
| 1 / 314160 | Pristine | 484,007,936 | 9,945,088 | 97.9453% |
| 1 / 314160 | 1% atomic saves | 484,069,376 | 11,055,104 | 97.7162% |
| 1 / 314160 | After churn | 484,589,568 | 11,902,976 | 97.5437% |
| 2 / 314161 | Pristine | 483,995,648 | 9,945,088 | 97.9452% |
| 2 / 314161 | 1% atomic saves | 484,040,704 | 10,874,880 | 97.7533% |
| 2 / 314161 | After churn | 484,700,160 | 11,853,824 | 97.5544% |

The installed package was built from checkout `4f6037a6bc901d41a1266811414b74e5c7161a54`.
The exercised checkout API source files were unchanged; managed-workspace additions
were outside this workload. The inline-test build hook rewrites installed Python
files, so the recorded SHA-256 values identify the installed runtime, not the
original source formatting. Installed runtime files remained unchanged throughout
measurement. Raw JSON, manifests, normalized-history checks, image allocation
receipts, and the plot are retained in the local `outputs/benchmark-expanded`
artifact directory. Its `qualification.json` contains SHA-256 hashes of every raw
JSON and JSONL receipt.

Qualification summary SHA-256:
`2431bda80d7e5eaf279cf009bc553c4942c22264f0e8985a3689d20ace7f1fac`.

## Reproduce

Run on macOS with APFS, Git, Python, and the repository's development environment.
The run creates six retained disk images. Each has a 2 GiB logical capacity.

```sh
git clone --bare --depth 1 --branch v3.13.0 \
  https://github.com/python/cpython.git /tmp/cpython-v3.13.0.git
uv run python -m benchmarks.space \
  --source /tmp/cpython-v3.13.0.git \
  --commit 60403a5409ff2c3f3b07dd2ca91a7a3e096839c7 \
  --output /tmp/cowtree-cpython-qualification \
  --leaves 4 --trials 3 --seed 314159 --capacity-gib 2
uv run python -m benchmarks.space_report \
  /tmp/cowtree-cpython-qualification \
  --output /tmp/cowtree-cpython-report
```

The output records the runtime source hashes, benchmark source hashes, platform,
commit, per-stage allocation and timings, full source manifests, operation
histories, and paired comparison receipts. A mismatch or unsettled allocation
counter stops the run rather than producing a passing report.

## Interpretation

Fleet savings compare each arm's stage allocation minus its own source-only
allocation. Total savings subtract each empty filesystem instead, so the source
checkout and Git object database are included once. Both include APFS metadata and
Git indexes. Sparse-image host allocation is reported separately because deleting
files can release container blocks without shrinking the backing image.

Three seeds and alternating order expose more workload variation than one pair.
They do not establish a universal savings percentage, confidence interval, sudden
power-loss safety, concurrent linearizability, or successful builds of the edited
source tree. The earlier Linux result remains a separate single-pair measurement.
