"""Release wheels strip test code while editable wheels retain source ownership."""

from pathlib import Path
import shutil
import zipfile

from hatchling.builders.wheel import WheelBuilder
import pytest


@pytest.mark.parametrize("version", ["standard", "editable"])
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
    builder = WheelBuilder(str(root))
    artifacts = list(builder.build(directory=str(tmp_path / "dist"), versions=[version]))
    assert len(artifacts) == 1
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
