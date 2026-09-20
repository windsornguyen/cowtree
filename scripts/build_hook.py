"""Strip release distributions while editable installs use the original package.

The upstream stripping hook force-includes Python copies even in editable
wheels. Those copies would hide the native extension beside the source files.
Keep the standard release hook and leave editable source ownership with Hatch.
"""

import inline_tests.hatch


BuildValue = str | bool | list[str] | tuple[str, ...] | dict[str, str]


class BuildHook(inline_tests.hatch.InlineTestsHook):
    def initialize(self, version: str, build_data: dict[str, BuildValue]) -> None:
        if version == "editable":
            return
        super().initialize(version, build_data)
