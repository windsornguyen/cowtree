Directory publication
======================

``publish_directory`` makes a completed staging directory visible at an absent
destination using exclusive rename. APFS uses ``renamex_np(RENAME_EXCL)`` and
Linux uses ``renameat2(RENAME_NOREPLACE)``. There is no ordinary rename fallback.
The operation preserves an existing destination, including an empty directory
or dangling symlink, and flushes the affected parent directories after success.

Source contents must already be durable. A failure flushing the parent after
rename leaves an uncertain publication that the caller must inspect during
recovery. Source and destination must be on the same filesystem.

Run ``cargo test -p cowtree-cli --all-features interrupted_fork`` for existing-path preservation
and concurrent admission. These are local filesystem tests, not power-loss tests.
The Linux operation is documented at https://man7.org/linux/man-pages/man2/rename.2.html.
