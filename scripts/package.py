"""Build the macOS arm64 one-folder CLI from a test-stripped wheel and release Rust."""

import argparse
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tempfile
import zipfile


PYINSTALLER = "6.22.3"


def prepare_python(repository: Path, work: Path) -> Path:
    """Install the stripped wheel and pinned packager into a private build environment."""
    wheels = work / "wheels"
    subprocess.run(  # noqa: S603
        ["uv", "build", "--python", sys.executable, "--wheel", "--out-dir", str(wheels)],  # noqa: S607
        cwd=repository,
        check=True,
    )
    (wheel,) = wheels.glob("*.whl")
    with zipfile.ZipFile(wheel) as archive:
        for name in archive.namelist():
            if name.endswith(".py") and b"from inline_tests import" in archive.read(name):
                raise RuntimeError(f"wheel contains inline tests: {name}")
    venv = work / "venv"
    subprocess.run(["uv", "venv", "--python", sys.executable, str(venv)], check=True)  # noqa: S603,S607
    python = venv / "bin/python"
    subprocess.run(  # noqa: S603
        [  # noqa: S607
            "uv",
            "pip",
            "install",
            "--python",
            str(python),
            f"pyinstaller=={PYINSTALLER}",
            str(wheel),
        ],
        check=True,
    )
    return python


def build(dist: Path) -> Path:
    if sys.platform != "darwin" or platform.machine() != "arm64":
        raise RuntimeError("this package target requires macOS arm64")
    repository = Path(__file__).resolve().parents[1]
    dist = dist.resolve()
    bundle = dist / "cowtree"
    if bundle.exists():
        raise FileExistsError(f"bundle already exists: {bundle}")
    build_root = repository / "build"
    build_root.mkdir(exist_ok=True)
    work = Path(tempfile.mkdtemp(prefix="package-", dir=build_root))
    python = prepare_python(repository=repository, work=work)
    environment = dict(os.environ)
    environment.pop("PYTHONPATH", None)
    environment["PYINSTALLER_CONFIG_DIR"] = str(work / "cache")
    environment["RUSTC_WRAPPER"] = ""
    environment["RUSTC_WORKSPACE_WRAPPER"] = ""
    environment["CARGO_TARGET_DIR"] = str(repository / "target")
    environment["CARGO_BUILD_BUILD_DIR"] = str(repository / "target")
    subprocess.run(
        ["cargo", "build", "--locked", "--release", "-p", "cowtree-metadata"],  # noqa: S607
        cwd=repository,
        env=environment,
        check=True,
    )
    subprocess.run(  # noqa: S603
        [
            str(python),
            "-I",
            "-m",
            "PyInstaller",
            "--onedir",
            "--clean",
            "--name",
            "cowtree",
            "--distpath",
            str(dist),
            "--workpath",
            str(work / "analysis"),
            "--specpath",
            str(work),
            "--copy-metadata",
            "cowtree",
            "--exclude-module",
            "inline_tests",
            str(repository / "scripts/package_entry.py"),
        ],
        cwd=work,
        env=environment,
        check=True,
    )
    shutil.copy2(repository / "target/release/cowtree-metadata", bundle / "cowtree-metadata")
    shutil.copy2(repository / "LICENSE", bundle / "LICENSE")
    shutil.copy2(repository / "docs/install.rst", bundle / "INSTALL.rst")
    return bundle


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dist", type=Path, default=Path("dist"))
    arguments = parser.parse_args()
    bundle = build(dist=arguments.dist)
    print(bundle)


if __name__ == "__main__":
    main()
