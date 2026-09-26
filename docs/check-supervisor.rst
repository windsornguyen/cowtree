Validation supervisor
======================

The native supervisor in ``crates/process`` owns a validation command's deadline
and process group. Its own group is isolated from terminal interrupts to the coordinator. The coordinating process can exit without removing that
deadline. The supervisor and command inherit the explicitly supplied operation
lock. Leader exit, timeout, and log limits all retire the owned process group before acknowledgement. Timeout returns 124. An observed
regular-file log larger than 128 MiB returns 125. This threshold can overshoot
between polls and is not a filesystem quota.

Commands must finish their work before exiting and must not daemonize. The
supervisor does not provide a process sandbox. Run ``cargo test -p cowtree-cli --test managed validation``.
