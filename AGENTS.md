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
- Fail closed with typed errors.
