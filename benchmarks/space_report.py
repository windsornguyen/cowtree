"""Render measured APFS allocation and paired savings without estimating shared bytes."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import json
from pathlib import Path
from statistics import median

from benchmarks.schema import Method


GIB = 1024**3
STAGES = ("pristine", "atomic_1pct", "round_2_append")
LABELS = ("Pristine", "1% atomic saves", "After churn")


@dataclass(frozen=True)
class Measurement:
    trial: int
    method: Method
    stage: str
    total: int
    incremental: int
    seconds: float


def read_measurements(root: Path) -> list[Measurement]:
    measurements: list[Measurement] = []
    metadata = json.loads((root / "metadata.json").read_text())
    for trial in range(metadata["config"]["trials"]):
        receipt = root / f"trial-{trial}-comparison.json"
        if not receipt.is_file() or json.loads(receipt.read_text()).get("matched") is not True:
            raise ValueError(f"incomplete paired run: trial {trial}")
        for method in Method:
            records = json.loads((root / f"trial-{trial}-{method.value}.json").read_text())
            if not records or records[-1]["stage"] != "detached":
                raise ValueError(f"incomplete arm: trial {trial}, {method.value}")
            spaces = {row["stage"]: row for row in records if "space" in row}
            empty = spaces["empty"]["space"]["used_bytes"]
            source = spaces["source"]["space"]["used_bytes"]
            for stage in STAGES:
                row = spaces[stage]
                used = row["space"]["used_bytes"]
                measurements.append(
                    Measurement(
                        trial, method, stage, used - empty, used - source, row["operation_seconds"]
                    )
                )
    return measurements


def savings(rows: list[Measurement], stage: str, *, total: bool) -> list[float]:
    selected = [row for row in rows if row.stage == stage]
    trials = sorted({row.trial for row in selected})
    ratios: list[float] = []
    for trial in trials:
        pair = {row.method: row for row in selected if row.trial == trial}
        git = pair[Method.GIT].total if total else pair[Method.GIT].incremental
        cow = pair[Method.COWTREE].total if total else pair[Method.COWTREE].incremental
        if git <= 0:
            raise ValueError("nonpositive Git footprint cannot define a savings ratio")
        ratios.append(100 * (1 - cow / git))
    return ratios


def render(root: Path, output: Path) -> None:
    import matplotlib.pyplot as plt

    rows = read_measurements(root=root)
    metadata = json.loads((root / "metadata.json").read_text())
    fig, axes = plt.subplots(1, 2, figsize=(10, 4.3), layout="constrained")
    fig.suptitle(
        f"Additional worktrees: {metadata['config']['leaves']}; "
        f"paired trials: {metadata['config']['trials']}"
    )
    for axis, total in zip(axes, (False, True), strict=True):
        for index, method in enumerate(Method):
            values = [
                median(
                    (row.total if total else row.incremental) / GIB
                    for row in rows
                    if row.stage == stage and row.method is method
                )
                for stage in STAGES
            ]
            axis.bar(
                [i + index * 0.36 for i in range(3)],
                values,
                width=0.36,
                label="Git worktree" if method is Method.GIT else "cowtree",
            )
        axis.set_xticks([i + 0.18 for i in range(3)], LABELS)
        axis.set_ylabel(
            "Allocated GiB" if metadata["config"]["trials"] == 1 else "Allocated GiB (median)"
        )
        axis.set_title("Source + fleet" if total else "Additional fleet only")
        axis.legend()
    output.mkdir(parents=True, exist_ok=True)
    fig.savefig(output / "space-savings.png", dpi=180)
    plt.close(fig)
    text = report_text(
        rows=rows,
        commit=metadata["config"]["commit"],
        trials=metadata["config"]["trials"],
        leaves=metadata["config"]["leaves"],
    )
    text += auxiliary_summary(root=root)
    (output / "report.md").write_text(text)


def report_text(rows: list[Measurement], commit: str, trials: int, leaves: int) -> str:
    lines = [
        "# Physical APFS worktree benchmark",
        "",
        f"{trials} paired trials; {leaves} additional worktrees; source `{commit}`.",
        "",
        "| Stage | Git fleet GiB | cowtree fleet GiB | Fleet saving | Total saving |",
        "|---|---:|---:|---:|---:|",
    ]
    for stage, label in zip(STAGES, LABELS, strict=True):
        git = median(
            row.incremental / GIB for row in rows if row.stage == stage and row.method is Method.GIT
        )
        cow = median(
            row.incremental / GIB
            for row in rows
            if row.stage == stage and row.method is Method.COWTREE
        )
        incremental = savings(rows=rows, stage=stage, total=False)
        total = savings(rows=rows, stage=stage, total=True)
        spread = f" ({min(incremental):.2f}-{max(incremental):.2f})" if trials > 1 else ""
        lines.append(
            f"| {label} | {git:.3f} | {cow:.3f} | {median(incremental):.2f}%{spread} "
            f"| {median(total):.2f}% |"
        )
    lines.extend(
        [
            "",
            "![Measured physical space](space-savings.png)",
            "",
            "Fleet savings = 1 - (cowtree stage - cowtree source) / "
            "(Git stage - Git source). Total savings subtract each empty filesystem "
            "instead. All numbers include APFS metadata and Git indexes. "
            "The source checkout and Git object database are counted once in total footprint. "
            "Later stages also include metadata written by earlier Git status checks.",
            "",
            "Container allocation is sampled after normal detach/reattach; "
            "the sparse image's allocation is retained separately as a host high-water footprint. "
            "Operation timings exclude checkpoints and verification.",
            "",
            "A seeded 1% sample of tracked regular C/header files receives atomic saves. "
            "Two rounds remove half the fleet (at least one tree), recreate it and append "
            "comments to another "
            "1% sample. The second round deletes one directory externally before API cleanup. "
            "Both arms must have identical operation results and state digests.",
            "",
            "Full manifests check source and every leaf before edits and after final churn. "
            "Intermediate checks hash every edited path plus approximately 128 clean paths "
            "in each tree. Checks cover content, modes, symlinks, HEAD, dirty paths and registry. "
            "This is sequential fault-injection testing; it does not prove crash consistency, "
            "concurrent linearizability, or successful builds of modified Linux sources.",
            "",
        ]
    )
    result = "\n".join(lines)
    return result


def auxiliary_summary(root: Path) -> str:
    lines = [
        "",
        "## Operation and backing-image evidence",
        "",
        "| Trial | Method | Create seconds | Peak image GiB | Cleanup residual MiB |",
        "|---|---|---:|---:|---:|",
    ]
    metadata = json.loads((root / "metadata.json").read_text())
    for trial in range(metadata["config"]["trials"]):
        for method in Method:
            records = json.loads((root / f"trial-{trial}-{method.value}.json").read_text())
            spaces = {row["stage"]: row for row in records if "space" in row}
            allocations = [row["space"]["image_allocated_bytes"] for row in spaces.values()]
            allocations.extend(
                row["image_allocated_bytes"] for row in records if row["stage"] == "detached"
            )
            high_water = max(allocations)
            residual = (
                spaces["cleanup"]["space"]["used_bytes"] - spaces["source"]["space"]["used_bytes"]
            )
            elapsed = spaces["pristine"]["operation_seconds"]
            lines.append(
                f"| {trial} | {method.value} | {elapsed:.2f} | "
                f"{high_water / GIB:.3f} | {residual / 1024**2:.3f} |"
            )
    lines.extend(
        [
            "",
            "Images retain allocation after deletion; no image compaction was performed. "
            "Cleanup residual includes retained filesystem and Git metadata after all "
            "leaf registrations and directories have been verified absent. "
            "Timing includes sparse-image I/O and unrelated host load; it does not establish "
            "native-volume speed. Trial 0 runs Git first; additional trials alternate arm order.",
            "",
        ]
    )
    result = "\n".join(lines)
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("results", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    render(root=args.results, output=args.output)


if __name__ == "__main__":
    main()
