Validation supervisor
======================

The internal ``cowtree.supervise`` process owns a validation command's deadline
and process group. The coordinating process can exit without removing that
deadline. The supervisor and command inherit the explicitly supplied operation
lock. Timeout returns 124 and terminates the owned process group. An observed
regular-file log larger than 128 MiB returns 125. This threshold can overshoot
between polls and is not a filesystem quota.

Commands must finish their work before exiting and must not daemonize. The
supervisor does not provide a process sandbox. Run ``uv run pytest tests/test_supervise.py``.
