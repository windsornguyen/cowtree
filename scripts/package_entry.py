"""Dispatch the standalone CLI and its embedded validation supervisor."""

import sys

from cowtree import cli, supervise


if __name__ == "__main__":
    if sys.argv[1:2] == ["--cowtree-supervise"]:
        del sys.argv[1]
        raise SystemExit(supervise.main())
    raise SystemExit(cli.main())
