# OpenCode v2 result integration

Verified against stable OpenCode v2.0.26 (`@opencode/plugin`, `@opencode/schema`, and `@opencode/client` 2.0.26). Use `tool.hook("execute.after")` to rewrite only text in `event.result.content`, preserving non-text parts, `output`, and `metadata` because structured results may be consumed by nested tools. Embed the plugin in Outrim and install it at `${XDG_CONFIG_HOME:-~/.config}/opencode/plugins/outrim.js` so OpenCode loads it without requiring user-side Bun/npm.
