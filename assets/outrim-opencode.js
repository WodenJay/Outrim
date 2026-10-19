// @bun
// adapters/opencode.ts
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
      request.finish(typeof response.text === "string" ? response.text : undefined);
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
      const finish = (result) => {
        if (settled)
          return;
        settled = true;
        signal?.removeEventListener("abort", abort);
        resolve(result);
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
      } finally {
        closingChild.kill();
      }
    }
  }
  return { process, close };
}

// adapters/opencode.ts
var BRIDGE_EXECUTABLE = "__OUTRIM_EXECUTABLE__";
var defaultSpawn = (executable, args, options) => nodeSpawn(executable, args, options);
function record(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value) ? value : undefined;
}
function exitCodeFrom(metadata) {
  const value = record(metadata);
  const code = value?.exit_code ?? value?.exitCode ?? value?.exit;
  return typeof code === "number" && Number.isInteger(code) && code >= -2147483648 && code <= 2147483647 ? code : null;
}
function textSelection(result) {
  if (typeof result.content === "string") {
    const original = result.content;
    return { texts: [original], apply: ([text]) => text ?? original };
  }
  if (Array.isArray(result.content)) {
    const content = result.content;
    const texts = content.flatMap((part) => {
      const item = record(part);
      return item?.type === "text" && typeof item.text === "string" ? [item.text] : [];
    });
    if (texts.length > 0) {
      return {
        texts,
        apply: (replacements) => {
          let index = 0;
          return content.map((part) => {
            const item = record(part);
            if (item?.type !== "text" || typeof item.text !== "string")
              return part;
            const text = replacements[index++];
            return typeof text === "string" ? { ...item, text } : part;
          });
        }
      };
    }
    if (content.length !== 0 || typeof result.output !== "string")
      return;
  } else if (result.content !== undefined || typeof result.output !== "string") {
    return;
  }
  const original = result.output;
  return { texts: [original], apply: ([text]) => text ?? original };
}
function sessionIdFrom(event) {
  const properties = record(event.properties);
  if (typeof properties?.sessionID === "string")
    return properties.sessionID;
  const info = record(properties?.info);
  return typeof info?.id === "string" ? info.id : undefined;
}
async function registerOpenCodePlugin(context, options = {}) {
  const bridge = createBridge("opencode", options.executable ?? BRIDGE_EXECUTABLE, options.spawnProcess ?? defaultSpawn);
  const pending = new Map;
  const eventController = new AbortController;
  let closed = false;
  function abortSession(session) {
    for (const controller of pending.get(session) ?? [])
      controller.abort();
  }
  async function handleResult(event) {
    if (closed || event.status !== "completed")
      return;
    const input = record(event.input);
    const result = event.result;
    if (!input || !result || inputReferencesOriginals(input))
      return;
    const selection = textSelection(result);
    if (!selection)
      return;
    const controller = new AbortController;
    let requests = pending.get(event.sessionID);
    if (!requests) {
      requests = new Set;
      pending.set(event.sessionID, requests);
    }
    requests.add(controller);
    try {
      const replacements = await Promise.all(selection.texts.map((text) => bridge.process(event.sessionID, {
        name: event.tool,
        input,
        source: sourceFor("opencode", event.tool, input),
        exit_code: exitCodeFrom(result.metadata)
      }, text, controller.signal)));
      if (closed || controller.signal.aborted)
        return;
      if (replacements.some((text, index) => text !== undefined && text !== selection.texts[index])) {
        event.result = { ...result, content: selection.apply(replacements) };
      }
    } finally {
      requests.delete(controller);
      if (requests.size === 0)
        pending.delete(event.sessionID);
    }
  }
  const registration = await context.tool.hook("execute.after", handleResult);
  let eventLoop;
  if (context.event) {
    eventLoop = (async () => {
      try {
        for await (const event of context.event.subscribe({ signal: eventController.signal })) {
          if (event.type !== "session.deleted" && event.type !== "session.execution.interrupted") {
            continue;
          }
          const session = sessionIdFrom(event);
          if (session)
            abortSession(session);
        }
      } catch {}
    })();
  }
  return async () => {
    if (closed)
      return;
    closed = true;
    eventController.abort();
    for (const requests of pending.values()) {
      for (const controller of requests)
        controller.abort();
    }
    bridge.close();
    await registration.dispose();
    await eventLoop;
  };
}

// adapters/opencode-plugin.ts
var OUTRIM_PLUGIN_MARKER = "outrim-managed: opencode-plugin";
var opencode_plugin_default = {
  id: "outrim",
  setup(context) {
    return registerOpenCodePlugin(context);
  }
};
export {
  OUTRIM_PLUGIN_MARKER,
  opencode_plugin_default as default
};
