Standalone submodules
=====================

Standalone ``add`` accepts uninitialized submodules by default. It preserves
the superproject's gitlinks, which record a child path and commit, and creates
empty directories at those paths. No submodule is fetched or initialized::

    cowtree add --committed ../worktree HEAD
    git -C ../worktree submodule status

An initialized source child is refused with ``submodule_initialized``. Choose
an explicit policy to create independent child repositories::

    cowtree add --committed --submodules materialize-pinned ../worktree HEAD

Use ``--submodules reject`` to retain the former blanket refusal. All policies
read the gitlinks from the requested commit in committed mode. Whether the
source child is initialized is still a filesystem admission check. Without
``--committed``, the source and initialized children must be clean at their pins.

Independent Git state
---------------------

``materialize-pinned`` creates a private Git repository at each child path.
It CoW-clones locally available object files, including packfiles, and creates
new indexes, refs, reflogs, and configuration. It never copies a source child's
``.git`` pointer or shares mutable Git metadata. Source object alternates and
special object-store entries are rejected. Shallow history and configured
promisor repositories must be made fully local first. Missing pinned objects produce
``submodule_unavailable`` before claiming the destination.

Checkout mode clones the child's tracked working files. Committed mode asks
Git to materialize the selected pin, independently of later source edits.
Nested children follow the same rule, up to 32 levels. A fresh child index
verifies the resulting bytes. Partial failures use the standalone transaction's
existing rollback, including deletion of a branch created by that operation.

Child repositories intentionally receive fresh local configuration. Source
hooks, credentials, remotes, and local filter settings are not copied. Git's
normal global configuration still applies. Git status and child commits work
normally, and commits in a child do not move the source child's HEAD.

Git activation and cleanup
--------------------------

Git requires activation settings to report a child as initialized. Cowtree
stores them in the destination's ``config.worktree``. On first use it enables
Git's ``extensions.worktreeConfig`` in the common repository configuration.
This feature setting persists after removal. Source submodule activation
settings and ``.git/modules`` contents remain unchanged.

If enabling the extension would activate an existing ``config.worktree`` or
change a repository using common ``core.worktree`` or ``core.bare=true``, the
operation refuses it. Configure Git's worktree extension explicitly before
retrying. Environment variables that redirect Git directories, indexes, or
objects are also refused for materialization.

``cowtree remove PATH`` verifies owned child repositories, their pins, and
tracked and untracked changes before using Git's required submodule removal
flag. Changed child commits or files require explicit ``--force``. A changed
Git-directory ownership boundary is refused even with force. Existing Git
worktree locks still require an explicit unlock.

``cowtree doctor PATH --json`` reports the worktree's recorded ``submodules``
policy. Outside a Cowtree worktree it reports the default. Policy records live
in worktree-private Git metadata, not among tracked source files.

This policy differs from managed ``workspace init --submodules materialize-pinned``.
Managed workspaces continue to capture dependency bytes as read-only source
without nested Git metadata. See `managed pinned dependencies <pinned-submodules.rst>`_.

Verification
------------

::

    COWTREE_EXPECT_SUPPORTED=1 cargo test -p cowtree-cli --test submodules

The real-Git cases cover default empty children, initialized-source refusal,
older pins, dirty source isolation, private metadata, nested children, missing
objects, hook interference, rollback, removal, and doctor output. Native CoW
support is required. See `Git submodules <https://git-scm.com/docs/gitsubmodules>`_
and `worktree configuration <https://git-scm.com/docs/git-worktree#_configuration_file>`_.
