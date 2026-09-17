"""Bound a validation process group even if its coordinating process exits."""

import argparse
import os
import signal
import stat
import subprocess
import sys
import time


class Arguments(argparse.Namespace):
    timeout: int
    descriptor: int
    command: list[str]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("--timeout", type=int, required=True)
    parser.add_argument("--descriptor", type=int, required=True)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args(namespace=Arguments())
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if args.timeout <= 0 or not command:
        parser.error("a positive timeout and command are required")
    process = subprocess.Popen(  # noqa: S603
        command,
        stdin=subprocess.DEVNULL,
        start_new_session=True,
        pass_fds=(args.descriptor,),
    )
    deadline = time.monotonic() + args.timeout
    try:
        while process.poll() is None:
            output = os.fstat(sys.stdout.fileno())
            if stat.S_ISREG(output.st_mode) and output.st_size > 128 * 1024 * 1024:
                print("validation log exceeded 128 MiB", file=sys.stderr)
                return 125
            if time.monotonic() >= deadline:
                print("validation deadline exceeded", file=sys.stderr)
                return 124
            time.sleep(0.05)
        result = process.returncode
        assert result is not None
        return result if result >= 0 else 128 - result
    finally:
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()


if __name__ == "__main__":
    raise SystemExit(main())
