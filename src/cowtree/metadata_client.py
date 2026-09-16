"""Run the required Rust metadata executable with a validated JSON protocol."""

from __future__ import annotations

from dataclasses import dataclass
import json
from pathlib import Path
import subprocess
from typing import cast

from cowtree.workspace_types import Json, MetadataError, RetryAction, WireRecord


@dataclass(frozen=True)
class MetadataClient:
    """Own an explicit executable and SQLite authority directory."""

    authority: Path
    binary: Path

    def call(self, command: dict[str, Json], kind: str) -> Json:
        """Execute one request; preserve authority errors and reject malformed responses."""
        try:
            request = json.dumps(command, ensure_ascii=True, allow_nan=False) + "\n"
            res = subprocess.run(
                args=[str(self.binary), str(self.authority)],
                input=request.encode("utf-8"),
                capture_output=True,
                check=False,
            )
        except FileNotFoundError as error:
            raise MetadataError(
                "metadata_unavailable", f"required metadata executable missing: {self.binary}"
            ) from error
        except OSError as error:
            raise MetadataError(
                "metadata_process", f"cannot execute {self.binary}: {error}"
            ) from error
        if res.returncode != 0:
            message = res.stderr.decode("utf-8", "replace").strip()
            raise MetadataError(
                "metadata_process", f"metadata exited with {res.returncode}: {message}"
            )
        try:
            value = cast(Json, json.loads(res.stdout))
        except (ValueError, UnicodeDecodeError) as error:
            raise MetadataError("invalid_response", "metadata returned invalid JSON") from error
        row = WireRecord.parse(value)
        status = row.text("status")
        if status == "error":
            try:
                retry = RetryAction(row.text("retry_action"))
            except ValueError as error:
                raise MetadataError("invalid_response", "unknown retry action") from error
            raise MetadataError(row.text("code"), row.text("message"), retry, row.record("details"))
        if status != "ok":
            raise MetadataError("invalid_response", f"unknown response status: {status}")
        output = row.record("output")
        actual = output.text("kind")
        if actual != kind:
            raise MetadataError("invalid_response", f"expected {kind}, received {actual}")
        if kind == "done":
            return None
        result = output.value("value")
        return result
