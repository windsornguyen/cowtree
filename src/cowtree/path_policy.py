"""Keep ignored files ephemeral unless a caller explicitly selects a cache."""

from pathlib import Path
import unicodedata

from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.exec import CommandRunner
from cowtree.git import GitRepository
from cowtree.tree_types import PathClass, PathPolicy
from cowtree.trees import classify, validate_policy


def check_aliases(paths: set[str]) -> None:
    """Require one spelling for each portable filesystem name."""
    spellings: dict[str, str] = {}
    for path in sorted(paths):
        parts = path.split("/")
        for length in range(1, len(parts) + 1):
            prefix = "/".join(parts[:length])
            key = unicodedata.normalize("NFC", prefix).casefold()
            previous = spellings.get(key)
            if previous is not None and previous != prefix:
                raise CowtreeError(
                    CowtreeErrorCode.INVALID_ARGUMENTS,
                    f"filesystem aliases: {previous!r}, {prefix!r}",
                )
            spellings[key] = prefix


def working_policy(source: Path, policy: PathPolicy) -> PathPolicy:
    """Bind capture policy to one complete Git checkout and its current ignore rules."""
    validate_policy(policy=policy)
    repository = GitRepository.discover(io=CommandRunner(), source=source)
    if repository.path != source.resolve():
        raise CowtreeError(
            CowtreeErrorCode.INVALID_ARGUMENTS, "capture source must be a checkout root"
        )
    sparse = repository.run(args=["config", "--bool", "core.sparseCheckout"], check=False)
    if sparse.returncode not in (0, 1):
        raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, sparse.stderr)
    if sparse.stdout.strip() == "true":
        raise CowtreeError(
            CowtreeErrorCode.SPARSE_CHECKOUT, "sparse workspace capture is unsupported"
        )
    tracked = repository.capture(args=["ls-files", "-z"]).split("\0")
    for path in tracked:
        if path and classify(path=path, policy=policy) is not PathClass.SOURCE:
            raise CowtreeError(
                CowtreeErrorCode.INVALID_ARGUMENTS,
                f"tracked source cannot be a cache or ephemeral: {path}",
            )
    ignored = repository.capture(
        args=["ls-files", "--others", "--ignored", "--exclude-standard", "--directory", "-z"]
    )
    prefixes = tuple(
        path.removesuffix("/")
        for path in ignored.split("\0")
        if path and not any(part.casefold() in (".git", ".cowtree") for part in path.split("/"))
    )
    check_aliases(
        paths={path for path in tracked if path}
        | set(prefixes)
        | set(policy.derived)
        | set(policy.ephemeral)
    )
    result = PathPolicy(
        derived=policy.derived, ephemeral=policy.ephemeral, ignored=(*policy.ignored, *prefixes)
    )
    return result
