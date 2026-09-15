from __future__ import annotations

import argparse
from dataclasses import dataclass
from pathlib import Path

from matplotlib import pyplot
from schema import BenchmarkResults, BenchmarkSummary, Method


@dataclass(frozen=True)
class PlotSeries:
    profiles: list[str]
    git: list[BenchmarkSummary]
    cowtree: list[BenchmarkSummary]


def main() -> int:
    parser = argparse.ArgumentParser(description="Render cowtree benchmark plots.")
    parser.add_argument("results", type=Path)
    parser.add_argument("--out-dir", type=Path, default=Path("benchmarks/plots"))
    args = parser.parse_args()

    data = BenchmarkResults.from_json_text(args.results.read_text())
    args.out_dir.mkdir(parents=True, exist_ok=True)
    plot_elapsed(data=data, out=args.out_dir / "elapsed_seconds.png")
    plot_time_ratio(data=data, out=args.out_dir / "time_ratio.png")
    plot_payload(data=data, out=args.out_dir / "payload_copied.png")
    plot_payload_avoided(data=data, out=args.out_dir / "payload_avoided.png")
    write_summary(data=data, out=args.out_dir / "summary.rst")
    print(args.out_dir)
    code = 0
    return code


def plot_elapsed(data: BenchmarkResults, out: Path) -> None:
    series = plot_series(data=data)
    x = list(range(len(series.profiles)))
    width = 0.36

    fig, ax = pyplot.subplots(figsize=(8, 4.8))
    ax.bar(
        [index - width / 2 for index in x],
        [row.median_elapsed_seconds for row in series.git],
        width,
        label="git worktree",
        color=method_color(method=Method.GIT),
    )
    ax.bar(
        [index + width / 2 for index in x],
        [row.median_elapsed_seconds for row in series.cowtree],
        width,
        label="cowtree",
        color=method_color(method=Method.COWTREE),
    )
    ax.set_title("Worktree creation time")
    ax.set_ylabel("median seconds")
    ax.set_xticks(x, labels=series.profiles)
    ax.legend()
    ax.grid(axis="y", alpha=0.25)
    fig.tight_layout()
    fig.savefig(out, dpi=180)
    pyplot.close(fig)


def plot_time_ratio(data: BenchmarkResults, out: Path) -> None:
    series = plot_series(data=data)
    ratios = [
        git.median_elapsed_seconds / cowtree.median_elapsed_seconds
        for git, cowtree in zip(series.git, series.cowtree, strict=True)
    ]

    fig, ax = pyplot.subplots(figsize=(8, 4.8))
    ax.bar(series.profiles, ratios, color="#059669")
    ax.axhline(1.0, color="#111827", linewidth=1)
    ax.set_title("Wall-time ratio")
    ax.set_ylabel("git worktree seconds / cowtree seconds")
    ax.grid(axis="y", alpha=0.25)
    for index, value in enumerate(ratios):
        ax.text(index, value, f"{value:.2f}x", ha="center", va="bottom")
    fig.tight_layout()
    fig.savefig(out, dpi=180)
    pyplot.close(fig)


def plot_payload(data: BenchmarkResults, out: Path) -> None:
    series = plot_series(data=data)
    x = list(range(len(series.profiles)))
    width = 0.36

    fig, ax = pyplot.subplots(figsize=(8, 4.8))
    ax.bar(
        [index - width / 2 for index in x],
        [mib(value=row.payload_copied_bytes) for row in series.git],
        width,
        label="git worktree",
        color=method_color(method=Method.GIT),
    )
    ax.bar(
        [index + width / 2 for index in x],
        [mib(value=row.payload_copied_bytes) for row in series.cowtree],
        width,
        label="cowtree",
        color=method_color(method=Method.COWTREE),
    )
    ax.set_title("Logical checkout payload copied")
    ax.set_ylabel("MiB")
    ax.set_xticks(x, labels=series.profiles)
    ax.legend()
    ax.grid(axis="y", alpha=0.25)
    fig.tight_layout()
    fig.savefig(out, dpi=180)
    pyplot.close(fig)


def plot_payload_avoided(data: BenchmarkResults, out: Path) -> None:
    series = plot_series(data=data)
    avoided = [
        mib(value=git.payload_copied_bytes - cowtree.payload_copied_bytes)
        for git, cowtree in zip(series.git, series.cowtree, strict=True)
    ]

    fig, ax = pyplot.subplots(figsize=(8, 4.8))
    ax.bar(series.profiles, avoided, color="#7c3aed")
    ax.set_title("Redundant checkout payload avoided")
    ax.set_ylabel("MiB avoided")
    ax.grid(axis="y", alpha=0.25)
    for index, value in enumerate(avoided):
        ax.text(index, value, f"{value:.1f} MiB", ha="center", va="bottom")
    fig.tight_layout()
    fig.savefig(out, dpi=180)
    pyplot.close(fig)


def write_summary(data: BenchmarkResults, out: Path) -> None:
    series = plot_series(data=data)
    lines = [
        "benchmark summary",
        "=================",
        "",
        f"created: {data.created_at}",
        f"platform: {data.platform.system} {data.platform.release} {data.platform.machine}",
        "",
        "+-----------+-----------+------------+------------+---------+-------------------+",
        "| profile   | worktrees | git sec    | cow sec    | ratio   | payload saved MiB |",
        "+===========+===========+============+============+=========+===================+",
    ]
    for git, cowtree in zip(series.git, series.cowtree, strict=True):
        ratio = git.median_elapsed_seconds / cowtree.median_elapsed_seconds
        saved = mib(value=git.payload_copied_bytes - cowtree.payload_copied_bytes)
        row = (
            f"| {git.profile:<9} | {git.worktrees:<9} | {git.median_elapsed_seconds:<10.3f} | "
            f"{cowtree.median_elapsed_seconds:<10.3f} | {ratio:<7.2f} | {saved:<17.1f} |"
        )
        lines.append(row)
        lines.append(
            "+-----------+-----------+------------+------------+---------+-------------------+"
        )
    lines.append("")
    out.write_text("\n".join(lines) + "\n")


def plot_series(data: BenchmarkResults) -> PlotSeries:
    profiles = [profile.name for profile in data.profiles]
    git = [find_summary(data=data, profile=profile, method=Method.GIT) for profile in profiles]
    cowtree = [
        find_summary(data=data, profile=profile, method=Method.COWTREE) for profile in profiles
    ]
    series = PlotSeries(profiles=profiles, git=git, cowtree=cowtree)
    return series


def find_summary(data: BenchmarkResults, profile: str, method: Method) -> BenchmarkSummary:
    for row in data.summary:
        if row.profile == profile and row.method is method:
            summary = row
            return summary
    raise KeyError(f"missing summary for {profile} {method.value}")


def method_color(method: Method) -> str:
    if method is Method.GIT:
        color = "#6b7280"
        return color
    if method is Method.COWTREE:
        color = "#2563eb"
        return color
    raise AssertionError(f"unknown method: {method}")


def mib(value: int | float) -> float:
    amount = float(value) / (1024 * 1024)
    return amount


if __name__ == "__main__":
    raise SystemExit(main())
