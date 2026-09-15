# Codebase Rules

- Do not use `Any`.
- Optimize for space efficiency first.
- Avoid redundant checked-out Git file payloads.
- Treat wall-time, CPU, and I/O speedups as secondary wins.
- Type every value that crosses a function boundary, including strings.
- Use enums for named cases.
- Use dataclasses or Pydantic models for structured values.
- Return structured values as classes, not loose dictionaries.
- Save return values to a named variable before returning them.
- Keep subprocess commands as `list[str]`; never build shell command strings.
- Use `uv` for package management, commands, lockfiles, and builds.
- Do not use `pip`, `poetry`, or `requirements.txt`.
- Fail closed with typed errors.

## Contributions

- Follow `CONTRIBUTING.rst` before opening a PR.
- New external contributors must open a Contribution interest issue and wait for
  a maintainer's vouch to appear in `VOUCHED.td` on the default branch.
- The repository owner, collaborators with write access, and contributors already
  vouched do not need a repeated contribution-interest issue.
- Do not edit `VOUCHED.td` in a contribution PR to grant yourself access.
- Link the approved issue in a new contributor's PR and report validation results.
- A vouch grants contribution access; it does not replace review or required checks.

## GitHub Actions

- Generate every workflow and local action through Hollywood Actions 0.0.5 or newer.
- Edit ``ci/*.ts``; never hand-edit generated YAML or action bundles.
- Run ``npm run ci generate`` and ``npm run ci check`` before publishing CI changes.
- Use structured commands or typed actions, never ``unsafeShell``.
