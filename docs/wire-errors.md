# JSON errors

The metadata executable accepts one JSON request per line. Every request yields
one response unless the process or its transport fails. Successful responses keep
`status: "ok"` and the existing tagged `output` object.

Failures preserve the existing top-level `message` and add stable codes and typed
context:

```json
{"status":"error","message":"workspace tip changed from 3 to 4","code":"tip_changed","retry_action":"reprepare","details":{"kind":"tip","expected":3,"actual":4}}
```

`message` is for people. Clients must use `code`, `retry_action`, and `details`;
message wording can change. `ErrorCode`, `RetryAction`, and `ErrorDetails` in the
Rust public API define the wire vocabulary. Context includes paths, leaf IDs,
request sequences, expected and observed versions or object hashes, operating
system error numbers, SQLite extended error numbers, and capacity categories.

## Recovery actions

| `retry_action` | Caller action |
| --- | --- |
| `none` | Correct the input or investigate the failure. Repeating it is not advised. |
| `retry_same_request` | Wait for the conflicting database user to finish, then repeat the same request with bounded retry policy. |
| `reprepare` | Prepare the existing proposal on the new tip, then commit the returned candidate. |
| `run_maintenance` | Run maintenance, resolve any blocked checkpoint, then retry the operation. |
| `resolve_conflict` | Choose the intended contents explicitly through conflict resolution. |
| `recover_installation` | Run recovery for the leaf's pending installation. |
| `sync_workspace` | Install the current tip before acquiring the affected resources. |

Only SQLite `BUSY` and `LOCKED` errors receive `retry_same_request`; corruption,
I/O errors, constraint failures, and other SQLite failures do not. A checkpoint
blocked by another connection also allows retry. Capacity errors expose
`details.resource`; history and WAL limits require maintenance, while object size,
leaf count, and other admission limits require a caller decision. `unsupported_filesystem`
identifies a missing native CoW primary and never requests a byte-copy fallback.

If a connection closes before a commit acknowledgement, query `result` with the
same request identity. An absent acknowledgement is not evidence of rollback.
Do not invent a new request sequence to retry an uncertain commit.

## Request framing

Malformed JSON, unknown operations, invalid typed input, and oversized lines use
`invalid_request` with `retry_action: "none"`. Requests are limited to 128 MiB,
including their newline. An oversized request is drained through its newline
without retaining the remaining bytes, then rejected. The next line is processed
as a new request. Invalid stored metadata uses `metadata_json` instead of
`invalid_request`.

`COWTREE_CRASH_AT` has no effect in a production build. Fault injection requires
the explicit Cargo feature and must not be enabled in production binaries.
