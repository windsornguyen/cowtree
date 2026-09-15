"""Compare physical APFS allocation under matched, seeded worktree churn."""

from __future__ import annotations

import argparse
from collections.abc import Callable
from dataclasses import asdict, dataclass, field
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import platform
import re
import time

from benchmarks.apfs_space import ManagedImage
from benchmarks.churn_workload import EditMode, Fleet, FleetConfig, MutationConfig
from benchmarks.schema import Method
import cowtree
from cowtree.exec import CommandRunner


@dataclass(frozen=True)
class Config:
    source: Path
    commit: str
    output: Path
    leaves: int
    trials: int
    seed: int
    capacity_gib: int


@dataclass
class Arm:
    config: Config
    trial: int
    method: Method
    image: ManagedImage
    records: list[dict[str, object]] = field(default_factory=list)

    def record(self, stage: str, operation: Callable[[], object] | None = None) -> None:
        start = time.monotonic()
        result = operation() if operation is not None else None
        elapsed = time.monotonic() - start
        checkpoint = time.monotonic()
        sample = self.image.sample()
        checkpoint_seconds = time.monotonic() - checkpoint
        row: dict[str, object] = {
            "stage": stage,
            "operation_seconds": elapsed,
            "checkpoint_seconds": checkpoint_seconds,
            "result": result,
            "space": asdict(sample),
        }
        self.records.append(row)
        self.save()
        print(f"{self.trial} {self.method.value} {stage}: {sample.used_bytes} bytes", flush=True)

    def save(self) -> None:
        path = self.config.output / f"trial-{self.trial}-{self.method.value}.json"
        path.write_text(json.dumps(self.records, indent=2) + "\n")

    def verify(self, fleet: Fleet, stage: str, *, full: bool = False) -> None:
        start = time.monotonic()
        result = fleet.verify(full=full)
        row: dict[str, object] = {
            "stage": stage,
            "verification_seconds": time.monotonic() - start,
            "verification": asdict(result),
        }
        self.records.append(row)
        self.save()

    def prepare_source(self) -> Fleet:
        cfg = self.config
        repo = self.image.path / "source"
        io = self.image.io
        start = time.monotonic()
        io.run(argv=["git", "clone", "--no-local", "--no-checkout", str(cfg.source), str(repo)])
        io.run(argv=["git", "-C", str(repo), "checkout", "--detach", cfg.commit])
        elapsed = time.monotonic() - start
        self.records.append({"stage": "source_setup", "operation_seconds": elapsed})
        fleet = Fleet(
            FleetConfig(
                repo=repo,
                root=self.image.path / "fleet",
                method=self.method,
                seed=cfg.seed + self.trial,
                history=cfg.output / f"trial-{self.trial}-{self.method.value}.jsonl",
            )
        )
        self.records.append(
            {
                "stage": "inventory",
                "tracked_files": len(fleet.inventory),
                "tracked_bytes": sum(entry.size for entry in fleet.inventory.values()),
                "eligible_c_header_files": len(fleet.eligible),
                "commit": fleet.commit,
            }
        )
        manifest = {name: asdict(entry) for name, entry in sorted(fleet.inventory.items())}
        target = cfg.output / f"trial-{self.trial}-{self.method.value}-manifest.json"
        target.write_text(json.dumps(manifest, sort_keys=True) + "\n")
        self.record(stage="source")
        return fleet

    def workload(self, fleet: Fleet) -> None:
        self.record(stage="pristine", operation=lambda: fleet.populate(count=self.config.leaves))
        self.verify(fleet=fleet, stage="pristine_verified", full=True)
        self.record(
            stage="atomic_1pct",
            operation=lambda: asdict(fleet.mutate(config=MutationConfig(round_id=0))),
        )
        self.verify(fleet=fleet, stage="atomic_verified")
        for round_id in range(1, 3):
            self.churn(fleet=fleet, round_id=round_id)
        self.record(stage="cleanup", operation=lambda: fleet.remove_random(count=len(fleet.active)))
        self.verify(fleet=fleet, stage="cleanup_verified", full=True)

    def churn(self, fleet: Fleet, round_id: int) -> None:
        removed: list[int] = []

        def remove() -> tuple[int, ...]:
            slots = fleet.remove_random(
                count=max(1, self.config.leaves // 2), external_loss=round_id == 2
            )
            removed.extend(slots)
            return slots

        self.record(stage=f"round_{round_id}_removed", operation=remove)
        self.verify(fleet=fleet, stage=f"round_{round_id}_survivors")
        self.record(
            stage=f"round_{round_id}_recreated",
            operation=lambda: fleet.recreate(slots=tuple(removed)),
        )
        self.verify(fleet=fleet, stage=f"round_{round_id}_recreated_verified")
        self.record(
            stage=f"round_{round_id}_append",
            operation=lambda: asdict(
                fleet.mutate(
                    config=MutationConfig(round_id=round_id, mode=EditMode.IN_PLACE_APPEND)
                )
            ),
        )
        self.verify(fleet=fleet, stage=f"round_{round_id}_verified", full=round_id == 2)


def run_arm(config: Config, trial: int, method: Method) -> None:
    with ManagedImage(
        config.output, f"trial-{trial}-{method.value}", config.capacity_gib, CommandRunner()
    ) as image:
        arm = Arm(config, trial, method, image)
        arm.record(stage="empty")
        fleet = arm.prepare_source()
        arm.workload(fleet=fleet)
    arm.records.append({"stage": "detached", "image_allocated_bytes": image.closed_image_bytes})
    arm.save()


def compare(config: Config, trial: int) -> None:
    arms = [
        json.loads((config.output / f"trial-{trial}-{method.value}.json").read_text())
        for method in Method
    ]
    for git, cow in zip(*arms, strict=True):
        if git["stage"] != cow["stage"]:
            raise RuntimeError("paired arms have different stages")
        if git.get("verification") != cow.get("verification"):
            raise RuntimeError(f"paired states differ at {git['stage']}")
        if git.get("result") != cow.get("result"):
            raise RuntimeError(f"paired operations differ at {git['stage']}")
    receipt = {"trial": trial, "matched_stages": len(arms[0]), "matched": True}
    path = config.output / f"trial-{trial}-comparison.json"
    path.write_text(json.dumps(receipt, indent=2) + "\n")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", required=True, type=Path, help="local bare Git input")
    parser.add_argument("--commit", required=True, help="full pinned source commit")
    parser.add_argument("--output", required=True, type=Path, help="new output directory")
    parser.add_argument("--leaves", type=int, default=4)
    parser.add_argument("--trials", type=int, default=3)
    parser.add_argument("--seed", type=int, default=1729)
    parser.add_argument("--capacity-gib", type=int, default=32)
    args = parser.parse_args()
    if platform.system() != "Darwin":
        parser.error("physical APFS image measurement requires macOS")
    cfg = Config(
        args.source.resolve(),
        args.commit,
        args.output.resolve(),
        args.leaves,
        args.trials,
        args.seed,
        args.capacity_gib,
    )
    if not 1 <= cfg.leaves <= 16 or not 1 <= cfg.trials <= 10 or not 1 <= cfg.capacity_gib <= 32:
        parser.error("require leaves 1..16, trials 1..10, capacity 1..32 GiB")
    if re.fullmatch(r"(?:[0-9a-f]{40}|[0-9a-f]{64})", cfg.commit) is None:
        parser.error("commit must be a full lowercase Git object ID")
    cfg.output.mkdir(parents=True, exist_ok=False)
    metadata = {
        "config": {**asdict(cfg), "source": str(cfg.source), "output": str(cfg.output)},
        "created_at": datetime.now(timezone.utc).isoformat(),
        "platform": platform.platform(),
        "python": platform.python_version(),
        "cowtree_module": cowtree.__file__,
        "benchmark_sources_sha256": {
            path.name: hashlib.sha256(path.read_bytes()).hexdigest()
            for path in sorted(Path(__file__).parent.glob("*.py"))
        },
        "runtime_sources_sha256": {
            path.name: hashlib.sha256(path.read_bytes()).hexdigest()
            for path in sorted(Path(cowtree.__file__).parent.glob("*.py"))
        },
        "runtime_git_head": CommandRunner()
        .run(
            argv=[
                "git",
                "-C",
                str(Path(cowtree.__file__).resolve().parents[2]),
                "rev-parse",
                "HEAD",
            ]
        )
        .stdout.strip(),
        "cpu": CommandRunner()
        .run(argv=["sysctl", "-n", "machdep.cpu.brand_string"])
        .stdout.strip(),
        "memory_bytes": CommandRunner().run(argv=["sysctl", "-n", "hw.memsize"]).stdout.strip(),
        "git_version": CommandRunner().run(argv=["git", "--version"]).stdout.strip(),
    }
    (cfg.output / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
    for trial in range(cfg.trials):
        methods = list(Method) if trial % 2 == 0 else list(reversed(Method))
        for method in methods:
            run_arm(config=cfg, trial=trial, method=method)
        compare(config=cfg, trial=trial)
    print("All paired operation plans and verified states match.", flush=True)


if __name__ == "__main__":
    main()
