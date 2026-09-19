"""Directory publication has one winner and preserves preexisting destinations."""

from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import pytest

from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.publication import publish_directory


@pytest.mark.parametrize("kind", ["empty", "populated", "symlink"])
def test_exclusive_publication_preserves_existing_destination(tmp_path: Path, kind: str) -> None:
    source = tmp_path / "source"
    source.mkdir()
    (source / "payload").write_bytes(b"new")
    target = tmp_path / "target"
    if kind == "symlink":
        target.symlink_to("missing")
    else:
        target.mkdir()
        if kind == "populated":
            (target / "sentinel").write_bytes(b"existing")
    with pytest.raises(CowtreeError) as caught:
        publish_directory(source=source, target=target)
    assert caught.value.code is CowtreeErrorCode.INVALID_ARGUMENTS
    assert (source / "payload").read_bytes() == b"new"
    if kind == "populated":
        assert (target / "sentinel").read_bytes() == b"existing"
    if kind == "symlink":
        assert target.is_symlink()


def test_concurrent_directory_publications_have_exactly_one_winner(tmp_path: Path) -> None:
    target = tmp_path / "target"
    sources = [tmp_path / f"source-{index}" for index in range(8)]
    for source in sources:
        source.mkdir()
        (source / "payload").write_text(source.name)

    def publish(source: Path) -> Path | None:
        try:
            publish_directory(source=source, target=target)
        except CowtreeError as error:
            if error.code is not CowtreeErrorCode.INVALID_ARGUMENTS:
                raise
            return None
        return source

    with ThreadPoolExecutor(max_workers=8) as executor:
        results = list(executor.map(publish, sources))
    winners = [source for source in results if source is not None]
    assert len(winners) == 1
    assert (target / "payload").read_text() == winners[0].name
    assert sum(source.exists() for source in sources) == 7
