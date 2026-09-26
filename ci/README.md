# Generated GitHub Actions

Hollywood **0.0.5** is pinned in `package.json` and `package-lock.json`. Edit the
TypeScript sources here; the workflow YAML, action metadata, entrypoints, and
bundles under `.github/` are generated outputs with Hollywood watermarks.

```sh
npm ci --ignore-scripts
npm run ci generate
npm run ci check
```

Use Node 24.11 or newer. `check` typechecks the sources, tests workflow coverage,
matrices, and privileged-action behavior, then regenerates both YAML and bundles.
It rejects generated differences and untracked outputs. Stage intentional
regeneration before running the local check; CI compares against committed files.

| Source             | Generated workflow or action                                             |
| ------------------ | ------------------------------------------------------------------------ |
| `ci.ts`            | Native builds, declarative schema, filesystem checks, protocol models, generated-file gate |
| `metadata.ts`      | SQLite Linux/macOS checks and minimum Rust compiler                      |
| `vouch.ts`         | Contributor eligibility and issue-based vouch management                 |
| `vouch-actions.ts` | Typed PR resolution and eligibility actions                              |
| `native.ts`        | Typed privileged filesystem test action                                  |

Simple commands use Hollywood's structured executable/argument API. Stateful
steps use typed action inputs and `ScriptExec`; no `unsafeShell` is used. The
runtime jobs compile Rust and execute native tests. Vouch and the
generation check use `ubuntu-24.04`.

The declarative-schema job builds the Apache-2.0 Atlas Community CLI from the
commit in `tools/atlas-revision.txt` with Go 1.26.5. It checks the generated SQL
and runs the real SQLite generation fixtures. The external source checkout has
credentials disabled; no Atlas account or proprietary binary is used.

## Trusted Vouch code

The eligibility workflow checks out the default branch before invoking local
actions or reading contribution policy, with checkout credentials disabled. It
never executes a contributor's checkout with the write token. Manual rechecks
still require an open PR at the exact selected same-repository SHA.

During this migration, `pull_request_target` runs the existing default-branch
workflow. The generated workflow and its bundled local actions become active
together when merged. A pre-merge manual dispatch of the new workflow cannot find
its new local actions in the old default branch; validate those actions locally
and use the existing PR eligibility check until the migration lands. Do not
change the trusted checkout to the contributor's head to bypass that boundary.

`vouch-manage` keeps its serialized issue-comment trigger, maintainer roles,
upstream action revision, and write permissions.
