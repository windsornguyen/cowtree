from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
from pathlib import Path
import platform
import shutil
import statistics
import tempfile
import time

from schema import BenchmarkProfile, BenchmarkResults, BenchmarkRun, BenchmarkSummary, Method, PlatformInfo

from cowtree.core import add_worktree, inspect_path
from cowtree.exec import CommandRunner
from cowtree.models import WorktreeAddRequest


QUICK_PROFILES = [
    BenchmarkProfile("basic", 1, 128, 4 * 1024, "one small worktree"),
    BenchmarkProfile("average", 4, 512, 8 * 1024, "several medium worktrees"),
    BenchmarkProfile("intensive", 12, 1024, 16 * 1024, "many larger worktrees"),
]

FULL_PROFILES = [
    BenchmarkProfile("basic", 1, 256, 4 * 1024, "one small worktree"),
    BenchmarkProfile("average", 8, 1024, 8 * 1024, "several medium worktrees"),
    BenchmarkProfile("intensive", 24, 2048, 16 * 1024, "many larger worktrees"),
]


def main() -> int:
    parser = argparse.ArgumentParser(description="Benchmark git worktree add vs cowtree add.")
    parser.add_argument("--preset", choices=("quick", "full"), default="quick")
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--out", type=Path, default=Path("benchmarks/results/latest.json"))
    args = parser.parse_args()

    profiles = QUICK_PROFILES if args.preset == "quick" else FULL_PROFILES
    results = run_benchmarks(profiles, runs=args.runs)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(results.to_json_text())
    print(args.out)
    code = 0
    return code


def run_benchmarks(profiles: list[BenchmarkProfile], *, runs: int) -> BenchmarkResults:
    runner = CommandRunner()
    started = datetime.now(timezone.utc).isoformat()
    records: list[BenchmarkRun] = []

    with tempfile.TemporaryDirectory(prefix="cowtree-bench-") as tmp:
        root = Path(tmp)
        report = inspect_path(root, runner)
        if not report.supported:
            raise SystemExit(f"CoW unavailable for benchmark root {root}: {report.reason}")

        for profile in profiles:
            repo = root / f"repo-{profile.name}"
            create_repo(repo, profile, runner)
            for iteration in range(runs):
                git_record = run_method(repo, root, profile, Method.GIT, iteration, runner)
                records.append(git_record)
                cowtree_record = run_method(repo, root, profile, Method.COWTREE, iteration, runner)
                records.append(cowtree_record)

    platform_info = PlatformInfo(
        system=platform.system(),
        release=platform.release(),
        machine=platform.machine(),
        python=platform.python_version(),
    )
    summary = summarize(records)
    results = BenchmarkResults(
        schema_version=1,
        created_at=started,
        platform=platform_info,
        profiles=profiles,
        runs=records,
        summary=summary,
    )
    return results


def create_repo(repo: Path, profile: BenchmarkProfile, runner: CommandRunner) -> None:
    repo.mkdir()
    runner.run(["git", "init", "-q", str(repo)])
    runner.run(["git", "-C", str(repo), "config", "user.email", "bench@example.com"])
    runner.run(["git", "-C", str(repo), "config", "user.name", "Benchmark"])

    for index in range(profile.files):
        directory = repo / "files" / f"{index // 256:04d}"
        directory.mkdir(parents=True, exist_ok=True)
        write_deterministic_file(directory / f"{index:06d}.dat", profile.bytes_per_file, f"{profile.name}:{index}")

    runner.run(["git", "-C", str(repo), "add", "."])
    runner.run(["git", "-C", str(repo), "commit", "-qm", "benchmark fixture"])


def write_deterministic_file(path: Path, size: int, seed: str) -> None:
    digest = hashlib.blake2b(seed.encode(), digest_size=64).digest()
    repeats, remainder = divmod(size, len(digest))
    with path.open("wb") as file:
        for _ in range(repeats):
            file.write(digest)
        file.write(digest[:remainder])


def run_method(
    repo: Path,
    root: Path,
    profile: BenchmarkProfile,
    method: Method,
    iteration: int,
    runner: CommandRunner,
) -> BenchmarkRun:
    target_root = root / f"{profile.name}-{method.value}-{iteration}"
    start = time.perf_counter()
    try:
        for index in range(profile.worktrees):
            target = target_root / f"wt-{index:03d}"
            create_worktree(repo, target, method, runner)
    finally:
        elapsed = time.perf_counter() - start
        remove_targets(repo, target_root, runner)

    payload_copied_bytes = profile.fleet_payload_bytes if method is Method.GIT else 0
    record = BenchmarkRun(
        profile=profile.name,
        method=method,
        iteration=iteration,
        elapsed_seconds=elapsed,
        worktrees=profile.worktrees,
        files=profile.files,
        tracked_bytes=profile.tracked_bytes,
        fleet_payload_bytes=profile.fleet_payload_bytes,
        payload_copied_bytes=payload_copied_bytes,
    )
    return record


def create_worktree(repo: Path, target: Path, method: Method, runner: CommandRunner) -> None:
    if method is Method.GIT:
        runner.run(["git", "-C", str(repo), "worktree", "add", "--detach", str(target), "HEAD"])
        return
    if method is Method.COWTREE:
        request = WorktreeAddRequest(path=target, source=repo, detach=True)
        add_worktree(request, runner)
        return
    raise AssertionError(f"unknown method: {method}")


def remove_targets(repo: Path, target_root: Path, runner: CommandRunner) -> None:
    if target_root.exists():
        for target in sorted(target_root.iterdir()):
            runner.run(["git", "-C", str(repo), "worktree", "remove", "--force", str(target)], check=False)
        shutil.rmtree(target_root, ignore_errors=True)


def summarize(records: list[BenchmarkRun]) -> list[BenchmarkSummary]:
    summaries: list[BenchmarkSummary] = []
    profile_names = sorted({record.profile for record in records})
    for profile in profile_names:
        for method in Method:
            rows = [record for record in records if record.profile == profile and record.method is method]
            if rows:
                summary = summarize_rows(rows)
                summaries.append(summary)
    return summaries


def summarize_rows(rows: list[BenchmarkRun]) -> BenchmarkSummary:
    timings = [row.elapsed_seconds for row in rows]
    first = rows[0]
    summary = BenchmarkSummary(
        profile=first.profile,
        method=first.method,
        runs=len(rows),
        median_elapsed_seconds=statistics.median(timings),
        min_elapsed_seconds=min(timings),
        max_elapsed_seconds=max(timings),
        worktrees=first.worktrees,
        files=first.files,
        tracked_bytes=first.tracked_bytes,
        fleet_payload_bytes=first.fleet_payload_bytes,
        payload_copied_bytes=first.payload_copied_bytes,
    )
    return summary


if __name__ == "__main__":
    raise SystemExit(main())
