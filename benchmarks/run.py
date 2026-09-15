from __future__ import annotations

import argparse
from dataclasses import dataclass
from datetime import datetime, timezone
import hashlib
from pathlib import Path
import platform
import shutil
import statistics
import tempfile
import time

from schema import (
    BenchmarkProfile,
    BenchmarkResults,
    BenchmarkRun,
    BenchmarkSummary,
    Method,
    PlatformInfo,
)

from cowtree.core import add_worktree, inspect_path
from cowtree.exec import CommandRunner
from cowtree.types import WorktreeAddRequest


QUICK_PROFILES = (
    BenchmarkProfile(
        name="basic",
        worktrees=1,
        files=128,
        bytes_per_file=4 * 1024,
        description="one small worktree",
    ),
    BenchmarkProfile(
        name="average",
        worktrees=4,
        files=512,
        bytes_per_file=8 * 1024,
        description="several medium worktrees",
    ),
    BenchmarkProfile(
        name="intensive",
        worktrees=12,
        files=1024,
        bytes_per_file=16 * 1024,
        description="many larger worktrees",
    ),
)

FULL_PROFILES = (
    BenchmarkProfile(
        name="basic",
        worktrees=1,
        files=256,
        bytes_per_file=4 * 1024,
        description="one small worktree",
    ),
    BenchmarkProfile(
        name="average",
        worktrees=8,
        files=1024,
        bytes_per_file=8 * 1024,
        description="several medium worktrees",
    ),
    BenchmarkProfile(
        name="intensive",
        worktrees=24,
        files=2048,
        bytes_per_file=16 * 1024,
        description="many larger worktrees",
    ),
)


def main() -> int:
    parser = argparse.ArgumentParser(description="Benchmark git worktree add vs cowtree add.")
    parser.add_argument("--preset", choices=("quick", "full"), default="quick")
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--out", type=Path, default=Path("benchmarks/results/latest.json"))
    args = parser.parse_args()

    profiles = QUICK_PROFILES if args.preset == "quick" else FULL_PROFILES
    results = run_benchmarks(profiles=profiles, runs=args.runs)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(results.to_json_text())
    print(args.out)
    code = 0
    return code


def run_benchmarks(profiles: tuple[BenchmarkProfile, ...], *, runs: int) -> BenchmarkResults:
    runner = CommandRunner()
    started = datetime.now(timezone.utc).isoformat()
    records: list[BenchmarkRun] = []

    with tempfile.TemporaryDirectory(prefix="cowtree-bench-") as tmp:
        root = Path(tmp)
        report = inspect_path(path=root, runner=runner)
        if not report.supported:
            raise SystemExit(f"CoW unavailable for benchmark root {root}: {report.reason}")

        for profile in profiles:
            repo = BenchmarkRepository(
                path=root / f"repo-{profile.name}", profile=profile, io=runner
            )
            repo.create()
            for iteration in range(runs):
                git_record = repo.measure(method=Method.GIT, iteration=iteration)
                records.append(git_record)
                cowtree_record = repo.measure(method=Method.COWTREE, iteration=iteration)
                records.append(cowtree_record)

    platform_info = PlatformInfo(
        system=platform.system(),
        release=platform.release(),
        machine=platform.machine(),
        python=platform.python_version(),
    )
    summary = summarize(records=records)
    results = BenchmarkResults(
        schema_version=1,
        created_at=started,
        platform=platform_info,
        profiles=list(profiles),
        runs=records,
        summary=summary,
    )
    return results


@dataclass(frozen=True)
class BenchmarkRepository:
    path: Path
    profile: BenchmarkProfile
    io: CommandRunner

    def create(self) -> None:
        self.path.mkdir()
        self.io.run(["git", "init", "-q", str(self.path)])
        self.io.run(["git", "-C", str(self.path), "config", "user.email", "bench@example.com"])
        self.io.run(["git", "-C", str(self.path), "config", "user.name", "Benchmark"])

        for index in range(self.profile.files):
            directory = self.path / "files" / f"{index // 256:04d}"
            directory.mkdir(parents=True, exist_ok=True)
            write_deterministic_file(
                path=directory / f"{index:06d}.dat",
                size=self.profile.bytes_per_file,
                seed=f"{self.profile.name}:{index}",
            )

        self.io.run(["git", "-C", str(self.path), "add", "."])
        self.io.run(["git", "-C", str(self.path), "commit", "-qm", "benchmark fixture"])

    def measure(self, method: Method, iteration: int) -> BenchmarkRun:
        target_root = self.path.parent / f"{self.profile.name}-{method.value}-{iteration}"
        start = time.perf_counter()
        try:
            for index in range(self.profile.worktrees):
                target = target_root / f"wt-{index:03d}"
                self.create_worktree(target=target, method=method)
        finally:
            elapsed = time.perf_counter() - start
            self.remove_targets(target_root=target_root)

        payload_copied_bytes = self.profile.fleet_payload_bytes if method is Method.GIT else 0
        record = BenchmarkRun(
            profile=self.profile.name,
            method=method,
            iteration=iteration,
            elapsed_seconds=elapsed,
            worktrees=self.profile.worktrees,
            files=self.profile.files,
            tracked_bytes=self.profile.tracked_bytes,
            fleet_payload_bytes=self.profile.fleet_payload_bytes,
            payload_copied_bytes=payload_copied_bytes,
        )
        return record

    def create_worktree(self, target: Path, method: Method) -> None:
        if method is Method.GIT:
            self.io.run(
                ["git", "-C", str(self.path), "worktree", "add", "--detach", str(target), "HEAD"]
            )
            return
        if method is Method.COWTREE:
            request = WorktreeAddRequest(path=target, source=self.path, detach=True)
            add_worktree(request=request, runner=self.io)
            return
        raise AssertionError(f"unknown method: {method}")

    def remove_targets(self, target_root: Path) -> None:
        if target_root.exists():
            for target in sorted(target_root.iterdir()):
                self.io.run(
                    ["git", "-C", str(self.path), "worktree", "remove", "--force", str(target)],
                    check=False,
                )
            shutil.rmtree(target_root, ignore_errors=True)


def write_deterministic_file(path: Path, size: int, seed: str) -> None:
    digest = hashlib.blake2b(seed.encode(), digest_size=64).digest()
    repeats, remainder = divmod(size, len(digest))
    with path.open("wb") as file:
        for _ in range(repeats):
            file.write(digest)
        file.write(digest[:remainder])


def summarize(records: list[BenchmarkRun]) -> list[BenchmarkSummary]:
    summaries: list[BenchmarkSummary] = []
    profile_names = sorted({record.profile for record in records})
    for profile in profile_names:
        for method in Method:
            rows = [
                record
                for record in records
                if record.profile == profile and record.method is method
            ]
            if rows:
                summary = summarize_rows(rows=rows)
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
