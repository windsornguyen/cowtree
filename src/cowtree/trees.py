"""Bind snapshot traversal and materialization to the shared Rust engine.

Python retains the existing entry dataclasses. Rust owns policy validation,
directory traversal, hashing, native cloning, and source-change checks.
Metadata-only capture still requires the caller's pinned-manifest check.
"""

from functools import partial
import json
from pathlib import Path

from pydantic import TypeAdapter

from cowtree import _libcowtree
from cowtree.core import invoke
from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.tree_types import CaptureMode, PathClass, PathPolicy, TreeEntry


ENTRIES = TypeAdapter(tuple[TreeEntry, ...])
CLASSIFICATION = TypeAdapter(PathClass)


def validate_policy(policy: PathPolicy) -> None:
    invoke(operation=partial(_libcowtree.validate_policy, policy=policy))


def classify(path: str, policy: PathPolicy) -> PathClass:
    record = invoke(operation=partial(_libcowtree.classify_path, path=path, policy=policy))
    result = CLASSIFICATION.validate_json(record)
    return result


def capture_mode(capture: CaptureMode) -> str:
    if not isinstance(capture, CaptureMode):
        raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, f"invalid capture mode: {capture!r}")
    return capture.value


def scan_tree(
    root: Path, policy: PathPolicy, *, capture: CaptureMode = CaptureMode.CONTENT
) -> tuple[TreeEntry, ...]:
    record = invoke(
        operation=partial(_libcowtree.scan_tree, root=root, policy=policy, mode=capture_mode(capture))
    )
    result = ENTRIES.validate_python(json.loads(record))
    return result


def clone_tree(source: Path, target: Path, policy: PathPolicy) -> tuple[TreeEntry, ...]:
    record = invoke(
        operation=partial(_libcowtree.clone_tree, source=source, target=target, policy=policy)
    )
    result = ENTRIES.validate_python(json.loads(record))
    return result


def populate_tree(
    source: Path, target: Path, policy: PathPolicy, *, capture: CaptureMode = CaptureMode.CONTENT
) -> tuple[TreeEntry, ...]:
    record = invoke(
        operation=partial(
            _libcowtree.populate_tree, source=source, target=target, policy=policy,
            mode=capture_mode(capture),
        )
    )
    result = ENTRIES.validate_python(json.loads(record))
    return result
