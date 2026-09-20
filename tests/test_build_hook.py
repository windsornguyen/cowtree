"""Release wheels strip test code while editable wheels retain source ownership."""

from pathlib import Path
import shutil
import tarfile
import zipfile

from hatchling.builders.sdist import SdistBuilder
from hatchling.builders.wheel import WheelBuilder
import pytest


@pytest.mark.parametrize("version", ["standard", "editable", "sdist"])
def test_build_modes_preserve_their_python_source_contract(tmp_path: Path, version: str) -> None:
    root = tmp_path / "project"
    package = root / "src/example"
    package.mkdir(parents=True)
    (package / "__init__.py").write_text('"""Build fixture."""\n')
    (package / "module.py").write_text(
        "from inline_tests import test\n\n"
        "def answer():\n    return 42\n\n"
        "@test\ndef answer_is_stable():\n    assert answer() == 42\n"
    )
    (root / "pyproject.toml").write_text(
        '[project]\nname = "example"\nversion = "0.0.1"\n'
        '[tool.hatch.build.targets.wheel]\npackages = ["src/example"]\n'
        '[tool.hatch.build.hooks.custom]\npath = "build_hook.py"\nsources = ["src"]\n'
    )
    source = Path(__file__).resolve().parents[1] / "scripts/build_hook.py"
    shutil.copy2(source, root / "build_hook.py")
    builder = SdistBuilder(str(root)) if version == "sdist" else WheelBuilder(str(root))
    build_version = "standard" if version == "sdist" else version
    artifacts = list(builder.build(directory=str(tmp_path / "dist"), versions=[build_version]))
    assert len(artifacts) == 1
    if version == "sdist":
        with tarfile.open(artifacts[0]) as archive:
            assert "example-0.0.1/pyproject.toml" in archive.getnames()
            name = "example-0.0.1/src/example/module.py"
            file = archive.extractfile(name)
            assert file is not None
            with file:
                code = file.read().decode()
            assert "def answer():" in code
            assert "answer_is_stable" not in code
            assert "example-0.0.1/example/module.py" not in archive.getnames()
        return
    with zipfile.ZipFile(artifacts[0]) as archive:
        if version == "standard":
            code = archive.read("example/module.py").decode()
            assert "def answer():" in code
            assert "answer_is_stable" not in code
            assert "inline_tests" not in code
        else:
            assert "example/module.py" not in archive.namelist()
            paths = [name for name in archive.namelist() if name.endswith(".pth")]
            assert len(paths) == 1
            assert str(root / "src") in archive.read(paths[0]).decode()
    assert "answer_is_stable" in (package / "module.py").read_text()


def test_source_archive_excludes_local_artifacts_even_beneath_ignored_paths(tmp_path: Path) -> None:
    root = tmp_path / ".cache/project"
    root.mkdir(parents=True)
    repository = Path(__file__).resolve().parents[1]
    for name in ["pyproject.toml", "README", "LICENSE", ".gitignore"]:
        shutil.copy2(repository / name, root / name)
    paths = [
        "src/cowtree/core.py",
        "src/cowtree/_libcowtree.so",
        "src/cowtree/__pycache__/core.pyc",
        "crates/libcowtree/src/lib.rs",
        "target/release/cowtree",
        "node_modules/local.js",
        ".venv/bin/python",
        "Cargo.toml",
        "Cargo.lock",
        "scripts/build_hook.py",
    ]
    for name in paths:
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("fixture\n")
    builder = SdistBuilder(str(root))
    selected = {file.distribution_path for file in builder.recurse_selected_project_files()}
    assert "src/cowtree/core.py" in selected
    assert "crates/libcowtree/src/lib.rs" in selected
    assert "Cargo.lock" in selected
    assert "scripts/build_hook.py" in selected
    assert not any(name.startswith(("target/", "node_modules/", ".venv/")) for name in selected)
    assert not any(name.endswith((".so", ".pyc")) for name in selected)
