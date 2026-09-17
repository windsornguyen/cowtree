"""Check the bounded workspace protocol with the pinned official TLC release."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import hashlib
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile


TLC_VERSION = "1.8.0"
TLC_SHA256 = "066cd246d87a388dfde0f04c3b506007f4c0cb4708a5b5396f0552a005eb75b5"
SPECS = Path(__file__).resolve().parent.parent / "specs"
TLC_JAR = SPECS.parent / "tools" / f"tla2tools-{TLC_VERSION}.jar"


class SpecCheckError(RuntimeError):
    """The configured checker did not produce the required verdict."""


@dataclass(frozen=True)
class ModelCase:
    name: str
    violation: str | None = None
    module: str = "Workspace"

    def check_result(self, log: Path, status: int) -> None:
        """Require successful exploration or the exact expected witness violation."""
        txt = log.read_text()
        if self.violation is None:
            if status != 0:
                raise SpecCheckError(f"{self.name}: TLC exited {status}; {log}")
            if "Model checking completed. No error has been found." not in txt:
                raise SpecCheckError(f"{self.name}: no completed exploration verdict; {log}")
        else:
            expected = f"Invariant {self.violation} is violated."
            if status != 12:
                raise SpecCheckError(f"{self.name}: expected invariant exit 12; {log}")
            if expected not in txt:
                raise SpecCheckError(f"{self.name}: missing expected {self.violation}; {log}")
            if "State 1:" not in txt:
                raise SpecCheckError(f"{self.name}: no counterexample trace; {log}")
            if not (log.parent / "counterexample.json").is_file():
                raise SpecCheckError(f"{self.name}: native JSON trace is missing; {log}")
        print(f"{self.name}: passed ({log})")
        for line in txt.splitlines():
            if "states generated" in line or "distinct states found" in line:
                print(line)


CASES = (
    ModelCase(name="PublicationRecovery", module="PublicationRecovery"),
    ModelCase(
        name="PublicationWrongValidation",
        module="PublicationRecovery",
        violation="ValidateBoundToId",
    ),
    ModelCase(
        name="PublicationPrematureGC", module="PublicationRecovery", violation="AckedRecoverable"
    ),
    ModelCase(name="FencedPublish", module="FencedPublish"),
    ModelCase(name="FencedPublishExpiry", module="FencedPublish"),
    ModelCase(name="FencedPublishInduction", module="FencedPublishInduction"),
    ModelCase(name="FencedPublishInductionTwoWriters", module="FencedPublishInduction"),
    ModelCase(name="MissingFence", module="FencedPublish", violation="PublishedWithAuthority"),
    ModelCase(name="WholeLeaf", module="FencedPublish", violation="UnmodifiedPathsPreserved"),
    ModelCase(name="StaleBaseWitness", module="FencedPublish", violation="NeverAcceptsStaleBase"),
    ModelCase(name="EpochLogReplayDiscard", module="EpochLogReplay", violation="ReplayIncomplete"),
    ModelCase(name="EpochLogReplayCommit", module="EpochLogReplay", violation="ReplayIncomplete"),
    ModelCase(name="Workspace"),
    ModelCase(name="StaleBase", violation="NoStaleBaseCommit"),
    ModelCase(name="StaleToken", violation="NoStaleTokenRejection"),
    ModelCase(name="BrokenFence", violation="AcceptedAuthority"),
    # Core: the protocol under Workspace and EpochLog. Reachable check, then the
    # finite invariant-seeded checks on the two cheap instances.
    ModelCase(name="Core", module="Core"),
    ModelCase(name="CoreInductive1", module="Core"),
    ModelCase(name="CoreInductive2", module="Core"),
    # EpochLog: fencing tokens over an explicit log. Tiny reachable model plus the
    # witness that a stale-base proposal commits without a merge.
    ModelCase(name="EpochLogTiny", module="EpochLog"),
    ModelCase(name="EpochLogPending", module="EpochLog"),
    ModelCase(name="EpochLogWitness", violation="NoStaleBaseCommit", module="EpochLog"),
    # CowTree: the first formulation; smoke plus one induction instance.
    ModelCase(name="CowTreeSmoke", module="CowTree"),
    ModelCase(name="CowTreeInductive2", module="CowTree"),
)


@dataclass(frozen=True)
class Checker:
    cache: Path
    java: str
    timeout_seconds: int

    @property
    def jar(self) -> Path:
        ret = self.cache / f"tla2tools-{TLC_VERSION}.jar"
        return ret

    def prepare(self) -> None:
        """Verify the vendored checker before populating or using its execution cache."""
        buf = TLC_JAR.read_bytes()
        if hashlib.sha256(buf).hexdigest() != TLC_SHA256:
            raise SpecCheckError(f"vendored tla2tools.jar failed SHA-256 validation: {TLC_JAR}")
        self.cache.mkdir(parents=True, exist_ok=True)
        if not self.jar.exists():
            with tempfile.NamedTemporaryFile(dir=self.cache, delete=False) as stream:
                temporary = Path(stream.name)
                try:
                    stream.write(buf)
                    stream.flush()
                    os.replace(temporary, self.jar)
                finally:
                    temporary.unlink(missing_ok=True)
        if hashlib.sha256(self.jar.read_bytes()).hexdigest() != TLC_SHA256:
            raise SpecCheckError(f"cached tla2tools.jar failed SHA-256 validation: {self.jar}")
        os.environ["TLA2TOOLS_JAR"] = str(self.jar)

    def run(self, case: ModelCase) -> None:
        """Retain each command, identity, complete log, and expected counterexample."""
        out = Path(tempfile.mkdtemp(prefix=f"{case.name}-", dir=self.cache))
        cfg = SPECS / f"{case.name}.cfg"
        model = SPECS / f"{case.module}.tla"
        shutil.copy2(cfg, out / cfg.name)
        dependencies = [model]
        if case.module == "FencedPublishInduction":
            dependencies.append(SPECS / "FencedPublish.tla")
        if case.module == "EpochLogReplay":
            dependencies.append(SPECS / "EpochLog.tla")
        for dependency in dependencies:
            shutil.copy2(dependency, out / dependency.name)
        cmd = [
            self.java,
            "-Xmx1g",
            "-XX:+UseParallelGC",
            "-cp",
            str(self.jar),
            "tlc2.TLC",
            "-workers",
            "1",
            "-fp",
            "0",
            "-coverage",
            "1",
            "-noGenerateSpecTE",
            "-dumpTrace",
            "json",
            "counterexample.json",
            "-config",
            cfg.name,
            model.name,
        ]
        log = out / "tlc.log"
        with log.open("w") as dst:
            dst.write(f"command: {cmd!r}\njar SHA-256: {TLC_SHA256}\n")
            dst.write((self.cache / "java-version.txt").read_text())
            for src in (*dependencies, cfg):
                dst.write(f"{src.name} SHA-256: {hashlib.sha256(src.read_bytes()).hexdigest()}\n")
            dst.flush()
            try:
                res = subprocess.run(  # noqa: S603
                    cmd,
                    cwd=out,
                    stdout=dst,
                    stderr=subprocess.STDOUT,
                    timeout=self.timeout_seconds,
                    check=False,
                )
            except subprocess.TimeoutExpired as err:
                dst.write(f"\nwall-time limit: {self.timeout_seconds} seconds\n")
                raise SpecCheckError(f"{case.name}: wall-time limit reached; {log}") from err
            dst.write(f"\nexit status: {res.returncode}\n")
        case.check_result(log=log, status=res.returncode)


def main() -> None:
    """Run each safety, witness, and mutation check with a bounded process budget."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--cache", type=Path, required=True, help="artifact directory outside checkout"
    )
    parser.add_argument("--java", default="java", help="Java 11+ executable")
    parser.add_argument("--timeout-seconds", type=int, default=180)
    parser.add_argument("--case", choices=[case.name for case in CASES])
    args = parser.parse_args()
    if args.timeout_seconds <= 0:
        parser.error("--timeout-seconds must be positive")
    java = shutil.which(args.java)
    if java is None:
        raise SpecCheckError(f"Java executable is missing: {args.java}")
    checker = Checker(
        cache=args.cache.resolve(),
        java=str(Path(java).resolve()),
        timeout_seconds=args.timeout_seconds,
    )
    if checker.cache.is_relative_to(SPECS.parent):
        parser.error("--cache must be outside the checkout")
    checker.prepare()
    # The release supports Java 8/11; use Java 11+ for this reproducible command.
    res = subprocess.run(  # noqa: S603
        [checker.java, "-version"],
        capture_output=True,
        text=True,
        timeout=10,
        check=True,
    )
    version = re.search(r'version "(?:1\.)?(\d+)', res.stdout + res.stderr)
    if version is None:
        raise SpecCheckError("Java version is not recognized")
    if int(version.group(1)) < 11:
        raise SpecCheckError("Java 11 or later is required")
    (checker.cache / "java-version.txt").write_text(res.stdout + res.stderr)
    print(res.stdout + res.stderr, end="")
    for case in CASES:
        if args.case is None or args.case == case.name:
            checker.run(case=case)


if __name__ == "__main__":
    main()
