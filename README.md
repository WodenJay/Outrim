# Outrim

Shared Rust core for tool-output ingress compression. The current slice handles
known build, test, and runtime logs plus search hit lists; host adapters
and remaining compression policies are separate implementation tickets.

## Development

A clean checkout needs only the Rust toolchain:

```sh
cargo build
cargo test
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
```

## Core process

Run `outrim bridge` to keep a core process connected over stdin/stdout. Each input
line is one JSON request; embedded newlines are JSON-escaped. Each output line is
one JSON response, flushed immediately. Diagnostics go exclusively to stderr.
Closing stdin ends the process after queued requests are handled.

Example request (the text is deliberately short and therefore stays unchanged):

```json
{"version":1,"id":"request-1","host":"pi","session":"session-1","tool":{"name":"bash","input":{"command":"cargo test"},"source":"test_log","exit_code":0},"text":"running tests\nall tests passed\n"}
```

`src/protocol.rs` is the serialization definition:

- `version`: protocol version, currently `1`, independent of the package version.
- `id`: caller-chosen string, echoed unchanged. Callers should use unique IDs for
  in-flight requests. The core processes each stream serially; multiple processes
  can safely share the current user's original-output directory.
- `host`: `pi` or `opencode`; `session`: opaque session identifier.
- `tool`: host tool name, original JSON input, explicit source, and optional integer
  `exit_code` (`null` if unavailable). Adapters should identify known sources rather
  than expecting the core to parse arbitrary shell commands.
- `source`: `source_code`, `configuration`, `search`, `build_log`, `test_log`,
  `runtime_log`, or `unknown`.
- `text`: the complete tool text actually received, not a command to re-execute.

A normal response contains `version`, `id`, `text`, `disposition`, `applied_rule`,
and `original_path`. Only a compressed response has a non-null applied rule
(`log_tail` or `search_hit_groups`) and original path. Dispositions currently produced are:

| Disposition | Result |
| --- | --- |
| `compressed` | Log tail or grouped search hits plus original-output path replaces the input |
| `protected` | Source/configuration is intentionally unchanged |
| `not_shorter` | Candidate including its hint is not strictly shorter |
| `no_match` | No currently implemented policy applies |
| `failed_open` | Unsupported protocol version or necessary storage failure; original text returned |

`recovery_skipped` is reserved for host recovery-input detection. These fields
describe this result only; this slice does not persist gain accounting.

Invalid JSON/schema requests (including non-UTF-8 lines) receive an error response
instead of processed text:
`{"version":1,"id":null,"error":"invalid_json"}` or `invalid_request`. A schema
error echoes a string `id` when available. The stream remains usable. Adapters
must retain the original result so transport/schema errors or process exit can
also fail open. An unsupported version in an otherwise valid request returns a
normal `failed_open` response with unchanged text.

## Policy and originals

Routing is ordered: explicit source/configuration and known source/configuration
file outputs are protected first. Known log sources retain exactly the last ten
lines, preserving their original line endings. Search results in `path:line:content`,
`path:line`, or `path:line:column:content` form are grouped by path in order of first
appearance; hits within each group retain their encounter order, any supplied
content, positions, and record line endings. A separator is inserted if grouping
moves an unterminated record before another group. Trailing blank lines are
retained. Search input paths describe the query scope, not returned source-file
contents. Context, heading, mixed, ambiguous, or otherwise
unrecognized search output, including path-only lists without hit positions, remains
unchanged. Unknown content also remains unchanged. No content-wide JSON guessing can
bypass source protection.

The candidate ends with the fixed hint:

```text
[outrim]: The full output is available at <absolute path>
```

The hint and any added newline count toward the final length. Replacement happens
only if that final text has strictly fewer Unicode codepoints than the original;
UTF-8 bytes and UTF-16 code units are not the comparison metric.

Before returning a replacement, the core writes the exact original bytes to a new,
collision-resistant `.txt` file under the current user's
`~/.outrim/outputs/<host>/<session-fingerprint>/`. Session directory names are
bounded fingerprints rather than raw IDs, so even long or path-like IDs are safe.
Fingerprints only group files: collision-safe filenames remain the identity and
readers keep exact paths rather than reconstructing them from session IDs. The binary
uses `HOME` on Unix and `USERPROFILE` on Windows. There is no output-directory
flag or policy configuration. Library callers pass the current user's home to
`Processor::new`; tests use isolated temporary homes.

Directories are owner-only (`0700` on Unix, protected owner-rights DACLs on
Windows); files are private (`0600` on Unix, inherited owner-only access on
Windows). Directory symlinks/reparse points within `.outrim` are rejected. Windows
volumes without persistent ACL support fail open rather than save unprotected
text. Files are flushed before exposing their paths. Read the path with an existing
file reader, grep, or shell to recover the exact snapshot. No dedicated recovery
tool is needed.

Restarting does not delete files. Forks retain existing references and save new
results independently; nothing copies or automatically cleans up old originals.
If preparation or saving fails, the input text is returned with no internal error
message injected into it.

## Organization

One Cargo package supplies a library and binary. Modules are added as needed, not
as placeholders for later tickets:

| Path | Responsibility |
| --- | --- |
| `src/main.rs` | Thin CLI dispatch and current-user home resolution |
| `src/bridge.rs` | Persistent newline-delimited JSON transport |
| `src/protocol.rs` | Versioned request/response serialization |
| `src/lib.rs` | Single-output orchestration and final candidate selection |
| `src/rules.rs` | Source protection and policy selection |
| `src/search.rs` | Search-hit parsing and path grouping |
| `src/storage.rs` | Host/session layout and non-overwriting original writes |
| `src/storage/windows.rs` | Native Windows private-directory access control |
| `tests/processing.rs` | Public single-output behavior and privacy checks |
| `tests/bridge.rs` | Real-process recovery, restart, concurrency, and failure checks |
