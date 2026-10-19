// @bun
// adapters/pi.ts
import { spawn as nodeSpawn } from "child_process";

// adapters/shared.ts
var TEST_COMMANDS = [
  /^\s*cargo\s+test\b/i,
  /^\s*(?:bun\s+(?:test|run\s+test)|npm\s+(?:test|run\s+test)|pnpm\s+(?:test|run\s+test)|yarn\s+(?:test|run\s+test))\b/i,
  /^\s*(?:pytest|go\s+test|jest|vitest)\b/i
];
var BUILD_COMMANDS = [
  /^\s*cargo\s+(?:build|check|clippy)\b/i,
  /^\s*(?:bun|npm|pnpm|yarn)\s+(?:build|run\s+build)\b/i,
  /^\s*(?:go\s+build|tsc|vite\s+build|cmake\s+--build|make)\b/i
];
var RUNTIME_COMMANDS = [
  /^\s*cargo\s+run\b/i,
  /^\s*(?:bun|npm|pnpm|yarn)\s+(?:start|run\s+(?:dev|start))\b/i,
  /^\s*(?:node|python(?:3(?:\.\d+)?)?|go)\s+(?:\S+|$)/i
];
function matchesAny(patterns, command) {
  return patterns.some((pattern) => pattern.test(command));
}
function sourceFor(host, toolName, input) {
  if (["find", "grep", "ls"].includes(toolName) || host === "opencode" && ["glob", "list"].includes(toolName))
    return "search";
  if (toolName !== "bash" && toolName !== "powershell")
    return "unknown";
  const command = input.command;
  if (typeof command !== "string")
    return "unknown";
  if (matchesAny(TEST_COMMANDS, command))
    return "test_log";
  if (matchesAny(BUILD_COMMANDS, command))
    return "build_log";
  if (matchesAny(RUNTIME_COMMANDS, command))
    return "runtime_log";
  return "unknown";
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
      if (response.version !== 1 || typeof response.id !== "string" || !pending.has(response.id)) {
        fail(failedChild);
        return;
      }
      const request = pending.get(response.id);
      pending.delete(response.id);
      request.finish(response);
    }
  }
  function start() {
    let started;
    try {
      started = spawnProcess(executable, ["bridge"], {
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
    const request = { version: 1, id, host, session, tool, text };
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
            appliedRule: response.applied_rule
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
  function recordDelivery(delivery, signal) {
    if (closed || signal?.aborted)
      return;
    const currentChild = child ?? start();
    if (!currentChild)
      return;
    const id = String(++nextId);
    const request = {
      type: "record_delivery",
      version: 1,
      id,
      disposition: delivery.disposition,
      applied_rule: delivery.appliedRule,
      before_chars: delivery.beforeChars,
      after_chars: delivery.afterChars
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
  return { process, recordDelivery, close };
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
    if (event.parentToolCallId || event.content.length !== 1 || event.content[0].type !== "text" || typeof event.content[0].text !== "string") {
      return;
    }
    const original = event.content[0];
    const text = original.text;
    const beforeChars = Array.from(text).length;
    if (inputReferencesOriginals(event.input)) {
      bridge.recordDelivery({
        disposition: "recovery_skipped",
        appliedRule: null,
        beforeChars,
        afterChars: beforeChars
      }, context.signal);
      return;
    }
    const processed = await bridge.process(context.sessionManager.getSessionId(), {
      name: event.toolName,
      input: event.input,
      source: sourceFor("pi", event.toolName, event.input),
      exit_code: exitCodeFrom(event.structuredContent)
    }, text, context.signal);
    if (processed === undefined) {
      bridge.recordDelivery({
        disposition: "failed_open",
        appliedRule: null,
        beforeChars,
        afterChars: beforeChars
      }, context.signal);
      return;
    }
    if (context.signal?.aborted)
      return;
    bridge.recordDelivery({
      disposition: processed.disposition,
      appliedRule: processed.appliedRule,
      beforeChars,
      afterChars: Array.from(processed.text).length
    }, context.signal);
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
