# OpenCode v2 result integration

Verified against stable OpenCode v2.0.26 (`@opencode/plugin`, `@opencode/schema`, and `@opencode/client` 2.0.26). Use `tool.hook("execute.after")` to rewrite only text in `event.result.content`, preserving non-text parts, `output`, and `metadata` because structured results may be consumed by nested tools.

OpenCode v2.0.26 Code Mode nested tool hooks reuse the outer `execute` call ID and session. Track calls by `(sessionID, callID)` and nesting depth from `execute.before` through `execute.after`; skip nested results and process only the outer final `execute` result. Clear a session's tracked calls on interruption or deletion, and clear all calls on plugin unload. This prevents altering data the running program consumes while still allowing compression of its final model-facing text. The nested-call correlation relies on observed v2.0.26 runtime behavior; the hook type has no explicit parent-call field.

Embed the plugin in Outrim and install it at `${XDG_CONFIG_HOME:-~/.config}/opencode/plugins/outrim.js` so OpenCode loads it without requiring user-side Bun/npm.
