security
========

Please report security issues privately to ``security@dedaluslabs.ai``.

Do not open a public issue for a vulnerability before maintainers have had a
chance to triage it.

Relevant surfaces
-----------------

``cowtree`` shells out through ``subprocess`` with ``list[str]`` argv only. It
does not accept shell command strings.

The project creates files, directories, symlinks, and Git worktrees. Reports
about path traversal, unexpected filesystem writes, unsafe symlink handling, or
incorrect cleanup after failed worktree creation are in scope.
