# Pinned TLC checker

`tla2tools-1.8.0.jar` is the unchanged upstream checker used for this repository's
24-case qualification and retained JSON trace fixtures. It is a build-tool input,
not a Python runtime dependency. The checker verifies these exact bytes before
using either the vendored file or its execution cache; no download path exists.

- Size: 4,492,834 bytes
- SHA-256: `066cd246d87a388dfde0f04c3b506007f4c0cb4708a5b5396f0552a005eb75b5`
- Embedded source revision: `078405c22df8037571860457b2d078e8281bd851`
- Embedded build timestamp: `2026-09-17T01:53:47.596Z`
- Original official release asset: `tlaplus/tlaplus`, asset `569238611`, tag `v1.8.0`
- [Exact upstream source](https://github.com/tlaplus/tlaplus/tree/078405c22df8037571860457b2d078e8281bd851)
- [Source archive](https://github.com/tlaplus/tlaplus/archive/078405c22df8037571860457b2d078e8281bd851.tar.gz)

The upstream `v1.8.0` tag is a moving master build. By 2026-09-17T03:26:45Z,
upstream had deleted asset `569238611` and replaced it with asset `569359548`:
4,492,966 bytes, SHA-256 `9d36716ffb5e49d1ba8fae4651eba59f3189887e12eb90e204a42d2e6e993fef`,
source revision `142d0ba85e54a937c0fc5e1503941bd4cf46b684`. Both its browser download
and asset API returned those new bytes. Cowtree's checksum check rejected them.
Vendoring preserves the already tested build instead of following the moving tag.

## Licenses and source

The upstream source [MIT license](https://github.com/tlaplus/tlaplus/blob/078405c22df8037571860457b2d078e8281bd851/LICENSE)
is copied verbatim to `licenses/TLA-LICENSE.txt`. All license and notice files
embedded in the unchanged JAR are also extracted under `licenses/`, retaining
their original paths. They include MIT, Eclipse Public License 2.0, Apache 2.0
and JLine notices. The original embedded copies remain intact.

The exact upstream source tree above includes TLC and its Commons Math sources.
Its bundled Eclipse LSP4J components are version 0.21.1; their corresponding
source is available from [Eclipse LSP4J](https://github.com/eclipse-lsp4j/lsp4j/tree/v0.21.1).
JLine components are version 3.25.0; source is available from
[JLine](https://github.com/jline/jline3/tree/jline-parent-3.25.0).
The upstream source tree's `tlatools/org.lamport.tlatools/lib` directory identifies
these bundled component versions. No component is modified here.

Replacing the JAR requires a reviewed source identity, updated checksum and
licenses, rerunning all configurations, and regenerating/reviewing trace fixtures.
