// @bun
// adapters/pi.ts
import { spawn as nodeSpawn } from "child_process";

// adapters/shared.ts
import { readFileSync } from "fs";
import { homedir } from "os";
import { join } from "path";
var TEST_COMMANDS = [
  /^\s*cargo\s+test(?=\s|$)/i,
  /^\s*(?:bun|npm|pnpm|yarn)\s+(?:test|run\s+test)(?=\s|$)/i,
  /^\s*(?:pytest|go\s+test|jest|vitest)(?=\s|$)/i
];
var BUILD_COMMANDS = [
  /^\s*cargo\s+(?:build|check|clippy)(?=\s|$)/i,
  /^\s*(?:bun|npm|pnpm|yarn)\s+(?:build|run\s+build)(?=\s|$)/i,
  /^\s*(?:go\s+build|tsc|vite\s+build|cmake\s+--build|make)(?=\s|$)/i
];
var RUNTIME_COMMANDS = [
  /^\s*cargo\s+run(?=\s|$)/i,
  /^\s*(?:bun|npm|pnpm|yarn)\s+(?:start|run\s+(?:dev|start))(?=\s|$)/i,
  /^\s*gh\s+run\s+watch(?=\s|$)/i
];
function matchesAny(patterns, command) {
  return patterns.some((pattern) => pattern.test(command));
}
function isShell(host, toolName) {
  return toolName === "bash" || toolName === "powershell" || host === "opencode" && toolName === "shell";
}
function isSimpleCommand(command, dialect) {
  const head = /^[ \t]*([A-Za-z0-9_./:-]+)(?=[ \t]|$)/.exec(command)?.[1]?.toLowerCase();
  if (!head || /(?:^|\/)(?:bash|sh|zsh|dash|fish|ksh|csh|tcsh|cmd|pwsh|powershell)(?:\.exe)?$/.test(head) || [
    "eval",
    "exec",
    "call",
    "start",
    "source",
    ".",
    "command",
    "builtin",
    "env",
    "sudo",
    "invoke-expression",
    "iex",
    "invoke-command",
    "icm",
    "if",
    "then",
    "else",
    "fi",
    "for",
    "while",
    "until",
    "do",
    "done",
    "case",
    "esac",
    "function",
    "foreach",
    "switch",
    "try",
    "trap",
    "time",
    "coproc"
  ].includes(head))
    return false;
  if (/[\0\u2018\u2019\u201c\u201d]/.test(command))
    return false;
  if (dialect === "generic" && /[%!^\r\n]/.test(command))
    return false;
  let quote;
  for (const char of command) {
    if (quote === "'") {
      if (char === "'")
        quote = undefined;
    } else if (dialect !== "powershell" && char === "\\" || char === "`" || char === "$") {
      return false;
    } else if (quote === '"') {
      if (char === '"')
        quote = undefined;
    } else if (char === "'" || char === '"') {
      if (dialect === "generic" && char === "'")
        return false;
      quote = char;
    } else if (`|&;<>()[]{}#@,!%^\r
\x00`.includes(char) || /\s/.test(char) && char !== " " && char !== "\t") {
      return false;
    }
  }
  return quote === undefined;
}
function shellSkipReason(host, toolName, input) {
  if (!isShell(host, toolName))
    return null;
  const command = input.command;
  if (typeof command !== "string" || !isSimpleCommand(command, toolName === "bash" || toolName === "powershell" ? toolName : "generic")) {
    return "compound_or_uncertain_shell_command";
  }
  return null;
}
function sourceFor(host, toolName, input) {
  if (["find", "grep", "ls"].includes(toolName) || host === "opencode" && ["glob", "list"].includes(toolName))
    return "search";
  if (!isShell(host, toolName) || shellSkipReason(host, toolName, input))
    return "unknown";
  const command = input.command;
  if (typeof command !== "string")
    return "unknown";
  if (/^[ \t]*(?:rg|grep|git[ \t]+grep)(?=[ \t]|$)/.test(command))
    return "search";
  if (matchesAny(TEST_COMMANDS, command))
    return "test_log";
  if (matchesAny(BUILD_COMMANDS, command))
    return "build_log";
  if (matchesAny(RUNTIME_COMMANDS, command))
    return "runtime_log";
  return "unknown";
}
function distinct(values) {
  return [...new Set(values.flatMap((value) => value === null ? [] : [value]))];
}
function processingFacts(results) {
  const processed = results.filter((result) => result !== undefined);
  const dispositions = new Set(processed.map((result) => result.disposition));
  const rules = distinct(processed.map((result) => result.appliedRule));
  const paths = distinct(processed.map((result) => result.originalPath));
  const reasons = distinct(processed.map((result) => result.reason));
  if (processed.length !== results.length)
    dispositions.add("failed_open");
  const outcome = dispositions.size > 1 ? "mixed" : dispositions.values().next().value ?? "failed_open";
  return {
    parts: results.map((result) => ({
      outcome: result?.disposition ?? "failed_open",
      applied_rule: result?.appliedRule ?? null,
      original_path: result?.originalPath ?? null,
      candidates: result?.candidates ?? [],
      reason: result?.reason ?? (result ? null : "bridge_unavailable")
    })),
    outcome,
    appliedRule: rules.length === 1 ? rules[0] ?? null : null,
    originalPath: paths.length === 1 ? paths[0] ?? null : null,
    candidates: processed.flatMap((result) => result.candidates),
    reason: reasons.length === 0 ? null : reasons.join("; ")
  };
}
function inputReferencesOriginals(value) {
  if (typeof value === "string") {
    const normalized = value.replaceAll("\\", "/");
    return /(?:^|[/'"`\s=:([{])\.outrim\/outputs(?:\/|$|[/'"`\s),\]}])/i.test(normalized);
  }
  if (Array.isArray(value))
    return value.some(inputReferencesOriginals);
  if (value !== null && typeof value === "object") {
    return Object.values(value).some(inputReferencesOriginals);
  }
  return false;
}
function createBridge(host, executable, spawnProcess) {
  let recording = false;
  try {
    recording = JSON.parse(readFileSync(join(homedir(), ".outrim", "recording.json"), "utf8")).recording === true;
  } catch {}
  let child;
  let nextId = 0;
  let buffer = "";
  let closed = false;
  const pending = new Map;
  function fail(failedChild) {
    if (child !== failedChild)
      return;
    child = undefined;
    buffer = "";
    for (const request of pending.values())
      request.finish(undefined);
    pending.clear();
    failedChild.kill();
  }
  function receive(failedChild, chunk) {
    if (child !== failedChild)
      return;
    buffer += chunk;
    while (true) {
      const newline = buffer.indexOf(`
`);
      if (newline < 0)
        return;
      const line = buffer.slice(0, newline);
      buffer = buffer.slice(newline + 1);
      let parsed;
      try {
        parsed = JSON.parse(line);
      } catch {
        fail(failedChild);
        return;
      }
      if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
        fail(failedChild);
        return;
      }
      const response = parsed;
      const request = typeof response.id === "string" ? pending.get(response.id) : undefined;
      if (response.version !== 2 || !request) {
        fail(failedChild);
        return;
      }
      pending.delete(response.id);
      request.finish(response);
    }
  }
  function start() {
    let started;
    try {
      started = spawnProcess(executable, ["bridge", "--recording", recording ? "on" : "off"], {
        stdio: ["pipe", "pipe", "ignore"],
        windowsHide: true
      });
    } catch {
      return;
    }
    child = started;
    started.stdout.setEncoding("utf8");
    started.stdout.on("data", (chunk) => receive(started, chunk));
    started.stdin.on("error", () => fail(started));
    started.on("error", () => fail(started));
    started.on("exit", () => fail(started));
    return started;
  }
  function process(session, tool, text, signal) {
    if (closed || signal?.aborted)
      return Promise.resolve(undefined);
    const currentChild = child ?? start();
    if (!currentChild)
      return Promise.resolve(undefined);
    const id = String(++nextId);
    const request = { version: 2, id, host, session, tool, text };
    let line;
    try {
      line = `${JSON.stringify(request)}
`;
    } catch {
      return Promise.resolve(undefined);
    }
    return new Promise((resolve) => {
      let settled = false;
      const finish = (response) => {
        if (settled)
          return;
        settled = true;
        signal?.removeEventListener("abort", abort);
        if (response && typeof response.text === "string" && typeof response.disposition === "string" && (response.applied_rule === null || typeof response.applied_rule === "string")) {
          resolve({
            text: response.text,
            disposition: response.disposition,
            appliedRule: response.applied_rule,
            originalPath: typeof response.original_path === "string" ? response.original_path : null,
            candidates: Array.isArray(response.candidates) ? response.candidates : [],
            reason: typeof response.reason === "string" ? response.reason : null
          });
        } else {
          resolve(undefined);
        }
      };
      const abort = () => finish(undefined);
      pending.set(id, { finish });
      signal?.addEventListener("abort", abort, { once: true });
      if (signal?.aborted) {
        pending.delete(id);
        finish(undefined);
        return;
      }
      try {
        currentChild.stdin.write(line, (error) => {
          if (error)
            fail(currentChild);
        });
      } catch {
        fail(currentChild);
      }
    });
  }
  function recordDelivery(original, outcome, signal) {
    if (closed || signal?.aborted)
      return;
    const currentChild = child ?? start();
    if (!currentChild)
      return;
    const texts = typeof original === "string" ? [original] : original;
    const beforeChars = texts.reduce((total, text) => total + Array.from(text).length, 0);
    const processed = typeof outcome === "object" ? outcome : undefined;
    const id = String(++nextId);
    const request = {
      type: "record_delivery",
      version: 2,
      id,
      disposition: processed?.disposition ?? (typeof outcome === "string" ? outcome : "failed_open"),
      applied_rule: processed?.appliedRule ?? null,
      before_chars: beforeChars,
      after_chars: processed ? Array.from(processed.text).length : beforeChars
    };
    let line;
    try {
      line = `${JSON.stringify(request)}
`;
    } catch {
      return;
    }
    pending.set(id, { finish: () => {} });
    try {
      currentChild.stdin.write(line, (error) => {
        if (error)
          fail(currentChild);
      });
    } catch {
      fail(currentChild);
    }
  }
  function recordObservation(context, original, final, facts) {
    if (closed || !recording)
      return;
    const currentChild = child ?? start();
    if (!currentChild)
      return;
    const id = String(++nextId);
    const request = {
      type: "record_observation",
      version: 2,
      id,
      host,
      session: context.session,
      tool_call_id: context.toolCallId,
      tool: context.tool,
      original_text: original,
      final_text: final,
      outcome: facts.outcome,
      applied_rule: facts.appliedRule ?? null,
      original_path: facts.originalPath ?? null,
      candidates: facts.candidates ?? [],
      reason: facts.reason ?? null,
      parts: facts.parts ?? []
    };
    let line;
    try {
      line = `${JSON.stringify(request)}
`;
    } catch {
      return;
    }
    pending.set(id, { finish: () => {} });
    try {
      currentChild.stdin.write(line, (error) => {
        if (error)
          fail(currentChild);
      });
    } catch {
      fail(currentChild);
    }
  }
  function close() {
    if (closed)
      return;
    closed = true;
    const closingChild = child;
    child = undefined;
    buffer = "";
    for (const request of pending.values())
      request.finish(undefined);
    pending.clear();
    if (closingChild) {
      try {
        closingChild.stdin.end();
      } catch {
        closingChild.kill();
      }
    }
  }
  return { process, recordDelivery, recordObservation, close };
}

// adapters/pi.ts
var OUTRIM_EXTENSION_MARKER = "outrim-managed: pi-extension";
var BRIDGE_EXECUTABLE = "__OUTRIM_EXECUTABLE__";
function exitCodeFrom(structuredContent) {
  if (structuredContent === null || typeof structuredContent !== "object")
    return null;
  const value = structuredContent.exit_code;
  return typeof value === "number" && Number.isInteger(value) && value >= -2147483648 && value <= 2147483647 ? value : null;
}
var defaultSpawn = (executable, args, options) => nodeSpawn(executable, args, options);
function registerPiExtension(pi, options = {}) {
  const bridge = createBridge("pi", options.executable ?? BRIDGE_EXECUTABLE, options.spawnProcess ?? defaultSpawn);
  pi.on("tool_result", async (event, context) => {
    const observation = {
      session: context.sessionManager.getSessionId(),
      toolCallId: event.toolCallId ?? null,
      tool: {
        name: event.toolName,
        input: event.input,
        source: sourceFor("pi", event.toolName, event.input),
        exit_code: exitCodeFrom(event.structuredContent)
      }
    };
    const [content] = event.content;
    if (event.parentToolCallId || event.content.length !== 1 || content?.type !== "text" || typeof content.text !== "string") {
      const texts = event.content.flatMap((part) => part.type === "text" && typeof part.text === "string" ? [part.text] : []);
      const text = texts.length ? texts.join(`
`) : null;
      bridge.recordObservation(observation, text, text, {
        outcome: "unsupported",
        reason: event.parentToolCallId ? "nested_tool_call" : "unsupported_content_shape"
      });
      return;
    }
    const original = content;
    const text = original.text;
    if (inputReferencesOriginals(event.input)) {
      bridge.recordObservation(observation, text, text, {
        outcome: context.signal?.aborted ? "cancelled" : "recovery_skipped",
        reason: "original_directory_in_arguments"
      });
      bridge.recordDelivery(text, "recovery_skipped", context.signal);
      return;
    }
    const shellReason = shellSkipReason("pi", event.toolName, event.input);
    if (shellReason) {
      bridge.recordObservation(observation, text, text, {
        outcome: context.signal?.aborted ? "cancelled" : "protected",
        reason: shellReason
      });
      bridge.recordDelivery(text, "protected", context.signal);
      return;
    }
    const processed = await bridge.process(observation.session, observation.tool, text, context.signal);
    bridge.recordObservation(observation, text, context.signal?.aborted ? text : processed?.text ?? text, {
      ...processingFacts([processed]),
      ...context.signal?.aborted ? { outcome: "cancelled", reason: "host_signal_aborted" } : {}
    });
    if (processed === undefined) {
      bridge.recordDelivery(text, undefined, context.signal);
      return;
    }
    if (context.signal?.aborted)
      return;
    bridge.recordDelivery(text, processed, context.signal);
    if (processed.text === text)
      return;
    return {
      content: [{ ...original, text: processed.text }],
      details: event.details,
      structuredContent: event.structuredContent,
      isError: event.isError,
      usage: event.usage
    };
  });
  pi.on("session_shutdown", () => bridge.close());
}
function piExtension(pi) {
  registerPiExtension(pi);
}
export {
  OUTRIM_EXTENSION_MARKER,
  piExtension as default,
  registerPiExtension
};
