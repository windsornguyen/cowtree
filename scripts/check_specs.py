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
import urllib.request


TLC_VERSION = "1.7.4"
TLC_SHA256 = "936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88"
TLC_URL = f"https://github.com/tlaplus/tlaplus/releases/download/v{TLC_VERSION}/tla2tools.jar"
SPECS = Path(__file__).resolve().parent.parent / "specs"


class SpecCheckError(RuntimeError):
    """The configured checker did not produce the required verdict."""


@dataclass(frozen=True)
class ModelCase:
    name: str
    violation: str | None = None

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
        print(f"{self.name}: passed ({log})")
        for line in txt.splitlines():
            if "states generated" in line or "distinct states found" in line:
                print(line)


CASES = (
    ModelCase(name="Workspace"),
    ModelCase(name="StaleBase", violation="NoStaleBaseCommit"),
    ModelCase(name="StaleToken", violation="NoStaleTokenRejection"),
    ModelCase(name="BrokenFence", violation="AcceptedAuthority"),
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

    def fetch(self) -> None:
        """Fetch the release from upstream, rejecting bytes that differ from the pin."""
        self.cache.mkdir(parents=True, exist_ok=True)
        if not self.jar.exists():
            # The download URL is a fixed HTTPS upstream release, never user input.
            with urllib.request.urlopen(TLC_URL, timeout=60) as src:  # noqa: S310
                buf = src.read()
            if hashlib.sha256(buf).hexdigest() != TLC_SHA256:
                raise SpecCheckError("upstream tla2tools.jar does not match the pinned SHA-256")
            self.jar.write_bytes(buf)
        if hashlib.sha256(self.jar.read_bytes()).hexdigest() != TLC_SHA256:
            raise SpecCheckError(f"cached tla2tools.jar failed SHA-256 validation: {self.jar}")
        os.environ["TLA2TOOLS_JAR"] = str(self.jar)

    def run(self, case: ModelCase) -> None:
        """Retain each command, identity, complete log, and expected counterexample."""
        out = Path(tempfile.mkdtemp(prefix=f"{case.name}-", dir=self.cache))
        cfg = SPECS / f"{case.name}.cfg"
        model = SPECS / "Workspace.tla"
        shutil.copy2(cfg, out / cfg.name)
        shutil.copy2(model, out / model.name)
        cmd = [
            self.java,
            "-Xmx512m",
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
            "-config",
            cfg.name,
            model.name,
        ]
        log = out / "tlc.log"
        with log.open("w") as dst:
            dst.write(f"command: {cmd!r}\njar SHA-256: {TLC_SHA256}\n")
            dst.write((self.cache / "java-version.txt").read_text())
            for src in (model, cfg):
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
    checker.fetch()
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
