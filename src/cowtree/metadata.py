"""Run one local metadata process with typed, bounded request/response records."""

from __future__ import annotations

from dataclasses import dataclass, field
import json
import math
import os
from pathlib import Path
import select
import subprocess
import time
from types import TracebackType

from pydantic import JsonValue, TypeAdapter, ValidationError

from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.metadata_types import (
    ErrorDetails,
    Failure,
    MetadataReason,
    Operation,
    Output,
    Response,
    RetryAction,
)


RESPONSE = TypeAdapter[Response](Response)
MAX_LINE_BYTES = 128 * 1024 * 1024


class MetadataError(CowtreeError):
    """A typed authority failure whose reason survives the process boundary."""

    def __init__(
        self, reason: MetadataReason, message: str, retry_action: RetryAction, details: ErrorDetails
    ) -> None:
        self.reason = reason
        self.retry_action = retry_action
        self.details = details
        super().__init__(CowtreeErrorCode.COMMAND_FAILED, message)


@dataclass
class Metadata:
    """Own a persistent JSON-line session with an explicitly selected executable."""

    root: Path
    binary: Path
    timeout_seconds: float = 60
    descriptors: tuple[int, ...] = ()
    process: subprocess.Popen[str] | None = field(default=None, init=False)

    def __enter__(self) -> Metadata:
        if not math.isfinite(self.timeout_seconds) or self.timeout_seconds <= 0:
            raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "timeout must be positive")
        if self.process is not None:
            raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "metadata session is open")
        try:
            self.process = subprocess.Popen(  # noqa: S603
                [str(self.binary.resolve()), str(self.root.resolve())],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                encoding="utf-8",
                pass_fds=self.descriptors,
            )
        except OSError as error:
            raise CowtreeError(
                CowtreeErrorCode.COMMAND_FAILED, f"metadata executable unavailable: {self.binary}"
            ) from error
        return self

    def __exit__(
        self,
        exception_type: type[BaseException] | None,
        exception: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        del exception_type, traceback
        process = self.process
        assert process is not None
        assert process.stdin is not None
        close_error: OSError | None = None
        try:
            process.stdin.close()
        except OSError as error:
            close_error = error
        try:
            code = process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
            code = 124
        finally:
            assert process.stdout is not None
            assert process.stderr is not None
            process.stdout.close()
            process.stderr.close()
            self.process = None
        if code != 0 and exception is None:
            raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, f"metadata exited {code}")
        if close_error is not None and exception is None:
            raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, str(close_error)) from close_error

    def call(self, operation: Operation, payload: dict[str, JsonValue] | None = None) -> Output:
        """Send one command; a returned failure never becomes an empty success."""
        process = self.process
        if process is None:
            raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "metadata session is closed")
        assert process.stdin is not None
        assert process.stdout is not None
        body = {"op": operation.value}
        request = json.dumps(body if payload is None else {**payload, **body}) + "\n"
        if len(request.encode()) > MAX_LINE_BYTES:
            raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "metadata request is too large")
        try:
            process.stdin.write(request)
            process.stdin.flush()
            response = RESPONSE.validate_json(self.read_response())
        except (OSError, ValidationError) as error:
            raise CowtreeError(
                CowtreeErrorCode.COMMAND_FAILED, f"metadata protocol failed: {error}"
            ) from error
        if isinstance(response, Failure):
            raise MetadataError(
                reason=response.code,
                message=response.message,
                retry_action=response.retry_action,
                details=response.details,
            )
        return response.output

    def read_response(self) -> bytes:
        """Enforce the response deadline even when a process emits a partial line."""
        process = self.process
        assert process is not None
        assert process.stdout is not None
        deadline = time.monotonic() + self.timeout_seconds
        line = bytearray()
        while not line.endswith(b"\n"):
            remaining = max(0, deadline - time.monotonic())
            ready, _, _ = select.select([process.stdout], [], [], remaining)
            if not ready:
                process.kill()
                raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, "metadata response timed out")
            block = os.read(process.stdout.fileno(), 65536)
            if not block or len(line) + len(block) > MAX_LINE_BYTES:
                process.kill()
                raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, "invalid metadata response")
            line.extend(block)
        result = bytes(line)
        return result
