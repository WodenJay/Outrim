# Host bridge and execution boundary

The bridge uses newline-delimited JSON on stdin/stdout. Every request carries
`version: 2` and a caller-generated string `id`; responses echo both. Existing
processing, delivery and observation messages remain unchanged.

## Execution-preparation message

```json
{"type":"rewrite","version":2,"id":"1","host":"pi","session":"session-id","tool_call_id":"call-id","unit_id":"call-id:0","recording":false,"dialect":"bash","command":"cargo test 'a b'"}
```

- `host`: existing host identity (`pi`, `opencode`, or `exec`).
- `session`, `tool_call_id`, `unit_id`: execution correlation, never target argv.
- `recording`: the adapter's startup snapshot, including after bridge restarts.
- `dialect`: `bash` or `powershell`; unknown values decline rewriting.
- `command`: the original shell command, not an already separated argv.

Response:

```json
{"version":2,"id":"1","command":null}
```

`command` is either a replacement shell string or `null` (keep the original).
Preparing a command does not execute it, save originals, or record samples.
Invalid envelopes use the bridge's existing error response. Adapters keep the
original on missing/invalid responses, process failure, cancellation or a
five-second rewrite deadline. There is no shell-result compression fallback.

## Ownership

```text
Pi tool_call → Rust rewrite → mutate existing input.command
                         ↓
              Pi native approval / sandbox / Bash execution
                         ↓
              outrim exec → target stdout and stderr
                         ↓
              per-stream processing / recovery / gain / diagnostics
Pi tool_result (shell) → unchanged, no second accounting
Pi tool_result (non-shell) → existing result adapter
```

Rust owns flat-list segmentation, literal parsing, eligibility, per-unit source
classification and shell quoting.
The adapter supplies host facts and changes only the command string; it does not
register a replacement execution tool. Pi's effective shell settings and built-in
tool provenance determine its dialect. A custom tool, unknown shell or command
prefix is not assumed to be Bash. The installed Pi adapter currently recognizes
only native built-in Bash; the shared bridge also supports callers that can
reliably identify PowerShell. OpenCode keeps its existing result-stage behavior.

Only bounded, literal external commands are candidates: recognized build/test/
runtime/search commands and selected read-only Git/file/JSON commands. Quotes,
literal spaces and dialect-specific escapes are parsed, not evaluated. Flat lists
support `;`, newlines, `&&` and `||` in Bash and PowerShell (conditional chains
require PowerShell 7 in the original command). Each eligible execution unit is
wrapped independently; ineligible units do not prevent safe siblings from being
wrapped. A compound unit uses `<supplied-unit-id>:<1-based-segment-index>`;
stdout/stderr share that unit identity but remain separate samples. Host, session,
tool-call and recording facts remain common; source is derived from each unit's
own argv, never from a sibling.

Expansion, globbing, unconfirmed interactive flags, launchers and explicit
`outrim exec` (including `--no-compress`) remain native. All members of a pipeline
stay native: no terminal command is currently independently proven safe for
capture. A redirected unit and an entire command-substitution consumer also stay
native, with their original bytes and consumer data intact. These do not prevent
safe units outside that pipeline/consumer from being wrapped. Unknown/non-flat
grammar declines the entire call: blocks, subshells, background jobs, comments,
here-documents/strings, ANSI-C/locale/typographic quotes, backtick substitutions,
PowerShell assignments and stop-parsing syntax, for example. This is deliberately
not a general shell parser.
Follow commands (`tail`) and recognized watch/debug/interactive flags stay native.
PowerShell accepts `& 'quoted executable'` as well as unquoted executable heads;
a quoted head without `&` is an expression and is declined. Doubled double-quotes
also stay native because their argv behavior depends on the PowerShell mode.

Bash retains its original list operators. PowerShell chain operands cannot be
`if` statements, so flat, left-associative `A && B || C` chains are lowered to
`A; if ($?) { B }; if (-not $?) { C }` in the shared rewriter. Skipped branches
preserve `$?`; no scriptblock invocation (which would mask native failure), new
scope, temporary shell variables, or second execution service is introduced.
Assignments are declined because they can capture a whole chain's output.

The replacement resolves each target in the actual shell before wrapping it.
Aliases, functions, builtins and cmdlets take the original-command branch.
Windows wrapping additionally requires a native `.exe`; MSYS extensionless
resolution is accepted only when it identifies the same `.exe` file. Scripts and
batch launchers otherwise remain native. The wrapper's absolute path is shell
quoted. Its internal `exec --context <hex>` option transports UTF-8 JSON encoded
as hex, avoiding legacy PowerShell's native-argument quote stripping. Context
contains `host`, `session`, `tool_call_id`, `unit_id`, `source`, and `recording`;
it is consumed before `--` and is never supplied to the target or its environment.

The existing execution boundary preserves cwd, environment, stdin, exit status
and process-tree cancellation. Only complete, nonempty UTF-8 streams become
samples, once per stream. Recovery reads and source/configuration protection
remain in the shared processor. Binary, over-capacity and direct streams retain
the execution boundary's existing bypass semantics.

## Verification seams

- `tests/rewrite.rs`: real bridge plus Bash/PowerShell 7 execution, literal argv,
  actual function/alias resolution, sequence/conditional branches and continuation
  lines, machine consumer files, explicit bypass, per-stream originals, source
  attribution and distinct unit identities within one tool call.
- `tests/pi-adapter.test.mjs`: pre-call mutation, fail-open/cancellation/deadline,
  no shell-result work, and non-shell adaptation.
- `scripts/check-pi.mjs`: installed bundle → real bridge → native shell, recording
  snapshot, two-stream accounting, recovery and non-shell regression.
- Real Pi 1.3.0 was also exercised offline with its installed extension loader,
  built-in Bash and an in-process scripted provider: automatic dialect detection,
  mutated arguments, native execution, exact original recovery, two correlated
  stream records, process-tree cancellation and subsequent session reuse passed.
  This host-specific verification uses no forced dialect or replacement tool.
