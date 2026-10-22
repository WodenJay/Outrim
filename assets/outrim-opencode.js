// @bun
// adapters/opencode.ts
import { spawn as nodeSpawn } from "child_process";

// adapters/shared.ts
import { readFileSync, realpathSync, statSync } from "fs";
import { homedir } from "os";
import { isAbsolute, join, relative } from "path";
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
function hostOutputFacts(host, value) {
  const details = value && typeof value === "object" ? value : {};
  const raw = host === "pi" ? details.truncation : details;
  const truncation = raw && typeof raw === "object" ? raw : {};
  const facts = {};
  if (typeof truncation.truncated === "boolean")
    facts.truncated = truncation.truncated;
  if (host === "pi") {
    for (const name of [
      "totalLines",
      "totalBytes",
      "outputLines",
      "outputBytes",
      "maxLines",
      "maxBytes"
    ]) {
      const number = truncation[name];
      if (typeof number === "number" && Number.isSafeInteger(number) && number >= 0)
        facts[name] = number;
    }
    if (truncation.truncatedBy === "bytes" || truncation.truncatedBy === "lines")
      facts.truncatedBy = truncation.truncatedBy;
    for (const name of ["lastLinePartial", "firstLineExceedsLimit"]) {
      if (typeof truncation[name] === "boolean")
        facts[name] = truncation[name];
    }
    for (const name of ["matchLimitReached", "resultLimitReached", "entryLimitReached"]) {
      const number = details[name];
      if (typeof number === "number" && Number.isSafeInteger(number) && number > 0)
        facts[name] = number;
    }
    if (typeof details.linesTruncated === "boolean")
      facts.linesTruncated = details.linesTruncated;
  }
  const limited = facts.truncated === true || facts.linesTruncated === true || ["matchLimitReached", "resultLimitReached", "entryLimitReached"].some((name) => (name in facts));
  return {
    completeness: limited ? "host_truncated" : facts.truncated === false ? "complete" : "unknown",
    host_truncation: Object.keys(facts).length ? facts : null
  };
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
function recoveryAttribution(tool, input) {
  if (!inputReferencesOriginals(input))
    return null;
  const unattributed = { attribution: "unattributed", original_paths: [] };
  if (!input || typeof input !== "object" || Array.isArray(input))
    return unattributed;
  const args = input;
  let paths;
  if (tool === "read") {
    const path = args.path ?? args.filePath ?? args.file_path;
    if (typeof path !== "string")
      return unattributed;
    paths = [path];
  } else if (["bash", "powershell", "shell"].includes(tool) && typeof args.command === "string") {
    const command = args.command;
    if (/[\\`$\r\n\0|&;<>()*?{}[\]]/.test(command.replaceAll("\\", "/")))
      return unattributed;
    const words = [];
    const pattern = /\s*(?:'([^']*)'|"([^"]*)"|([^\s'"\\]+))(?=\s|$)/gy;
    let end = 0;
    for (const match of command.matchAll(pattern)) {
      if (match.index !== end)
        return unattributed;
      words.push(match[1] ?? match[2] ?? match[3] ?? "");
      end = match.index + match[0].length;
    }
    if (command.slice(end).trim())
      return unattributed;
    const program = words.shift();
    if (program !== "cat" && program !== "head")
      return unattributed;
    if (program === "head" && words[0] === "-n" && /^\d+$/.test(words[1] ?? ""))
      words.splice(0, 2);
    if (program === "head" && /^-\d+$/.test(words[0] ?? ""))
      words.shift();
    if (words[0] === "--")
      words.shift();
    if (words.some((word) => word.startsWith("-")))
      return unattributed;
    paths = words;
  } else
    return unattributed;
  try {
    const root = realpathSync(join(homedir(), ".outrim", "outputs"));
    return paths.length > 0 && paths.every((path) => {
      if (!isAbsolute(path))
        return false;
      const resolved = realpathSync(path);
      const within = relative(root, resolved);
      return within !== "" && !within.startsWith("..") && !isAbsolute(within) && statSync(resolved).isFile();
    }) ? { attribution: "confirmed", original_paths: [...new Set(paths)] } : unattributed;
  } catch {
    return unattributed;
  }
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
  function exchange(payload, signal, timeoutMs) {
    if (closed || signal?.aborted)
      return Promise.resolve(undefined);
    const currentChild = child ?? start();
    if (!currentChild)
      return Promise.resolve(undefined);
    const id = String(++nextId);
    const request = {
      ...payload,
      version: 2,
      id,
      ...payload.type === "process_parts" ? {
        request: { ...payload.request, id }
      } : {}
    };
    let line;
    try {
      line = `${JSON.stringify(request)}
`;
    } catch {
      return Promise.resolve(undefined);
    }
    return new Promise((resolve) => {
      let settled = false;
      let timer;
      const finish = (response) => {
        if (settled)
          return;
        settled = true;
        clearTimeout(timer);
        signal?.removeEventListener("abort", abort);
        resolve(response);
      };
      const abort = () => finish(undefined);
      pending.set(id, { finish });
      signal?.addEventListener("abort", abort, { once: true });
      if (signal?.aborted) {
        pending.delete(id);
        finish(undefined);
        return;
      }
      if (timeoutMs !== undefined)
        timer = setTimeout(() => fail(currentChild), timeoutMs);
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
  async function selectPolicy(session, tool, cwd, signal) {
    try {
      const request = {
        version: 2,
        id: "",
        host,
        session,
        tool,
        text: "",
        ...cwd === undefined ? {} : { context: { cwd } }
      };
      const response = await exchange({ type: "resolve_policy", request }, signal, 5000);
      return response?.selection ?? "off";
    } catch {
      return "off";
    }
  }
  async function processParts(session, tool, texts, cwd, signal, selection) {
    const request = {
      version: 2,
      id: "",
      host,
      session,
      tool,
      text: "",
      ...cwd === undefined ? {} : { context: { cwd } }
    };
    const response = await exchange({ type: "process_parts", request, texts, selection }, signal);
    if (!Array.isArray(response?.parts) || response.parts.length !== texts.length)
      return texts.map(() => {
        return;
      });
    return response.parts.map(processedResult);
  }
  function processedResult(response) {
    if (!response || typeof response.text !== "string" || typeof response.disposition !== "string" || !(response.applied_rule === null || typeof response.applied_rule === "string"))
      return;
    return {
      text: response.text,
      disposition: response.disposition,
      appliedRule: response.applied_rule,
      originalPath: typeof response.original_path === "string" ? response.original_path : null,
      candidates: Array.isArray(response.candidates) ? response.candidates : [],
      reason: typeof response.reason === "string" ? response.reason : null
    };
  }
  async function process(session, tool, text, signal, cwd, selection) {
    if (selection !== undefined)
      return (await processParts(session, tool, [text], cwd, signal, selection))[0];
    const payload = {
      version: 2,
      id: "",
      host,
      session,
      tool,
      text,
      ...cwd === undefined ? {} : { context: { cwd } }
    };
    return processedResult(await exchange({ ...payload }, signal));
  }
  async function rewrite(session, toolCallId, command, dialect, signal, tool) {
    const response = await exchange({
      type: "rewrite",
      tool,
      host,
      session,
      tool_call_id: toolCallId,
      unit_id: `${toolCallId}:0`,
      recording,
      dialect,
      command
    }, signal, 5000);
    const rewritten = typeof response?.command === "string" ? response.command : undefined;
    recordWrapping(session, toolCallId, "shell", rewritten ? "planned" : "rejected", response?.reason ?? (response ? "unsupported_command" : "bridge_unavailable"), dialect);
    return rewritten;
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
      tool: facts.metadataOnly ? { ...context.tool, input: null } : context.tool,
      original_text: facts.metadataOnly ? null : original,
      final_text: facts.metadataOnly ? null : final,
      metadata_only: facts.metadataOnly ?? false,
      diagnostics: {
        adapter_build: "9a3b2d01c1a92f532b0b00f002d17b230911ac52d44c2e486d34c23f412cf796",
        output_origin: "host_result",
        completeness: "unknown",
        observed_chars: original === null ? null : Array.from(original).length,
        observed_bytes: original === null ? null : Buffer.byteLength(original, "utf8"),
        ...context.diagnostics,
        ...facts.diagnostics
      },
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
  function recordRecovery(session, toolCallId, texts, evidence) {
    exchange({
      type: "record_recovery",
      host,
      session,
      tool_call_id: toolCallId,
      event_id: JSON.stringify([host, session, toolCallId, "recovery"]),
      characters: texts.reduce((count, text) => count + Array.from(text).length, 0),
      ...evidence
    });
  }
  function recordWrapping(session, call, tool, state, reason, dialect, actual) {
    recordObservation({
      session,
      toolCallId: call,
      tool: { name: tool, input: null, source: "unknown", exit_code: null }
    }, null, null, {
      outcome: "unsupported",
      metadataOnly: true,
      reason,
      diagnostics: {
        wrapping: state,
        attempt_id: JSON.stringify([host, session, call]),
        requested_dialect: dialect,
        actual_dialect: actual ?? null,
        output_origin: null
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
      } catch {
        closingChild.kill();
      }
    }
  }
  return {
    rewrite,
    selectPolicy,
    process,
    processParts,
    recordDelivery,
    recordObservation,
    recordRecovery,
    recordWrapping,
    close
  };
}

// adapters/opencode.ts
var BRIDGE_EXECUTABLE = "__OUTRIM_EXECUTABLE__";
var defaultSpawn = (executable, args, options) => nodeSpawn(executable, args, options);
function record(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value) ? value : undefined;
}
function isShell2(tool) {
  return ["shell", "bash", "powershell"].includes(tool);
}
function shellDialect(shell) {
  const name = shell.split(/[/\\\\]/).pop()?.toLowerCase().replace(/\.exe$/, "");
  if (name === "bash")
    return "bash";
  if (name === "pwsh" || name === "powershell")
    return "powershell";
  return "unknown";
}
function exitCodeFrom(metadata) {
  const value = record(metadata);
  const code = value?.exit;
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
  const originalInputs = new Map;
  const callDirectories = new Map;
  const callPolicies = new Map;
  async function sessionDirectory(sessionID) {
    try {
      const session = await context.session?.get?.({ sessionID });
      if (typeof session?.location.directory !== "string")
        return;
      return session.location.directory;
    } catch {
      return;
    }
  }
  const callKey = (session, call) => JSON.stringify([session, call]);
  const activeCallsBySession = new Map;
  const activeCodeModeCalls = new Map;
  const dialects = new Map;
  const rewrites = new Map;
  const eventController = new AbortController;
  let closed = false;
  function abortSession(session) {
    for (const controller of activeCallsBySession.get(session) ?? [])
      controller.abort();
    activeCodeModeCalls.delete(session);
    dialects.delete(session);
    for (const [command, rewrite] of rewrites) {
      if (rewrite.session === session)
        rewrites.delete(command);
    }
  }
  async function handleBefore(event) {
    if (closed)
      return;
    const originalInput = record(event.input);
    const key = callKey(event.sessionID, event.id);
    const nested = activeCodeModeCalls.get(event.sessionID)?.has(event.id);
    if (!nested) {
      originalInputs.set(key, structuredClone(event.input));
      const cwd = context.session?.get ? await sessionDirectory(event.sessionID) : undefined;
      if (closed)
        return;
      if (cwd !== undefined)
        callDirectories.set(key, cwd);
      if (!isShell2(event.tool)) {
        callPolicies.set(key, await bridge.selectPolicy(event.sessionID, {
          name: event.tool,
          input: originalInputs.get(key) ?? null,
          source: sourceFor("opencode", event.tool, originalInput ?? {}),
          exit_code: null
        }, cwd));
      }
    }
    if (event.tool === "execute") {
      let sessionCalls = activeCodeModeCalls.get(event.sessionID);
      if (!sessionCalls) {
        sessionCalls = new Map;
        activeCodeModeCalls.set(event.sessionID, sessionCalls);
      }
      sessionCalls.set(event.id, (sessionCalls.get(event.id) ?? 0) + 1);
      return;
    }
    const input = record(event.input);
    const dialect = dialects.get(event.sessionID)?.get(event.agent) ?? "unknown";
    if (event.tool !== "shell" || !context.shell || dialect === "unknown" || typeof input?.command !== "string" || activeCodeModeCalls.get(event.sessionID)?.has(event.id)) {
      bridge.recordWrapping(event.sessionID, event.id, event.tool, "not_attempted", dialect === "unknown" ? "unknown_dialect" : "source_not_applicable", dialect);
      return;
    }
    const original = input.command;
    const controller = new AbortController;
    let calls = activeCallsBySession.get(event.sessionID);
    if (!calls) {
      calls = new Set;
      activeCallsBySession.set(event.sessionID, calls);
    }
    calls.add(controller);
    try {
      const command = await bridge.rewrite(event.sessionID, event.id, original, dialect, controller.signal, {
        name: event.tool,
        input: originalInputs.get(callKey(event.sessionID, event.id)) ?? event.input,
        source: sourceFor("opencode", event.tool, originalInput ?? {}),
        exit_code: null
      });
      if (closed || controller.signal.aborted || input.command !== original || !command || command === original) {
        if (command)
          bridge.recordWrapping(event.sessionID, event.id, event.tool, "withdrawn", controller.signal.aborted ? "cancelled" : "changed_input", dialect);
        return;
      }
      bridge.recordWrapping(event.sessionID, event.id, event.tool, "adopted", null, dialect);
      rewrites.set(command, { original, dialect, session: event.sessionID, call: event.id });
      input.command = command;
    } finally {
      calls.delete(controller);
      if (calls.size === 0)
        activeCallsBySession.delete(event.sessionID);
    }
  }
  async function handleResult(event) {
    const codeModeSessionCalls = activeCodeModeCalls.get(event.sessionID);
    const codeModeDepth = codeModeSessionCalls?.get(event.id) ?? 0;
    const isCodeModeCall = codeModeDepth > 0;
    if (event.tool === "execute" && codeModeSessionCalls && isCodeModeCall) {
      if (codeModeDepth === 1) {
        codeModeSessionCalls.delete(event.id);
        if (codeModeSessionCalls.size === 0)
          activeCodeModeCalls.delete(event.sessionID);
      } else {
        codeModeSessionCalls.set(event.id, codeModeDepth - 1);
      }
    }
    for (const [command, rewrite] of rewrites) {
      if (rewrite.session === event.sessionID && rewrite.call === event.id)
        rewrites.delete(command);
    }
    if (closed)
      return;
    const key = callKey(event.sessionID, event.id);
    const ownsCall = !isCodeModeCall || event.tool === "execute" && codeModeDepth === 1;
    const policy = ownsCall ? callPolicies.get(key) : undefined;
    const cwd = (ownsCall ? callDirectories.get(key) : undefined) ?? (context.session?.get ? await sessionDirectory(event.sessionID) : undefined);
    const recoveryInput = (ownsCall ? originalInputs.get(key) : undefined) ?? event.input;
    if (ownsCall) {
      callDirectories.delete(key);
      originalInputs.delete(key);
      callPolicies.delete(key);
    }
    const recoverySelection = event.result ? textSelection(event.result) : undefined;
    const attribution = recoveryAttribution(event.tool, recoveryInput);
    if (attribution && event.status === "completed" && recoverySelection && !isCodeModeCall && (exitCodeFrom(event.result?.metadata) ?? 0) === 0) {
      bridge.recordRecovery(event.sessionID, event.id, recoverySelection.texts, attribution);
    }
    if (isShell2(event.tool)) {
      bridge.recordObservation({
        session: event.sessionID,
        toolCallId: event.id,
        diagnostics: hostOutputFacts("opencode", event.result?.metadata),
        tool: {
          name: event.tool,
          input: null,
          source: "unknown",
          exit_code: exitCodeFrom(event.result?.metadata)
        }
      }, recoverySelection?.texts.join(`
`) ?? null, null, {
        outcome: event.status === "cancelled" ? "cancelled" : "unsupported",
        reason: event.status === "completed" ? "shell_result_observed" : "host_error",
        metadataOnly: true
      });
      return;
    }
    const input = record(recoveryInput);
    const result = event.result;
    const selection = result ? textSelection(result) : undefined;
    const observation = {
      session: event.sessionID ?? null,
      toolCallId: event.id ?? null,
      diagnostics: hostOutputFacts("opencode", result?.metadata),
      tool: {
        name: event.tool,
        input: recoveryInput ?? null,
        source: sourceFor("opencode", event.tool, input ?? {}),
        exit_code: exitCodeFrom(result?.metadata)
      }
    };
    const originalText = selection ? selection.texts.join(`
`) : null;
    const unsupportedReason = event.status !== "completed" ? `host_status:${event.status}` : isCodeModeCall && (event.tool !== "execute" || codeModeDepth > 1) ? "code_mode_nested" : !input ? "unsupported_arguments" : !result ? "missing_result" : !selection ? "unsupported_content_shape" : null;
    if (unsupportedReason || !input || !result || !selection) {
      bridge.recordObservation(observation, originalText, originalText, {
        outcome: event.status === "cancelled" ? "cancelled" : "unsupported",
        reason: unsupportedReason
      });
      return;
    }
    if (inputReferencesOriginals(input)) {
      bridge.recordObservation(observation, originalText, originalText, {
        outcome: "recovery_skipped",
        reason: "original_directory_in_arguments"
      });
      bridge.recordDelivery(selection.texts, "recovery_skipped");
      return;
    }
    const controller = new AbortController;
    let bridgeSessionCalls = activeCallsBySession.get(event.sessionID);
    if (!bridgeSessionCalls) {
      bridgeSessionCalls = new Set;
      activeCallsBySession.set(event.sessionID, bridgeSessionCalls);
    }
    bridgeSessionCalls.add(controller);
    try {
      const processedResults = await bridge.processParts(event.sessionID, observation.tool, selection.texts, cwd, controller.signal, policy);
      if (closed || controller.signal.aborted) {
        bridge.recordObservation(observation, originalText, originalText, {
          ...processingFacts(processedResults),
          outcome: "cancelled",
          reason: "host_signal_aborted"
        });
        return;
      }
      selection.texts.forEach((original, index) => {
        bridge.recordDelivery(original, processedResults[index], controller.signal);
      });
      const replacements = processedResults.map((processed) => processed?.text);
      bridge.recordObservation(observation, originalText, selection.texts.map((text, index) => replacements[index] ?? text).join(`
`), processingFacts(processedResults));
      if (replacements.some((text, index) => text !== undefined && text !== selection.texts[index])) {
        event.result = { ...result, content: selection.apply(replacements) };
      }
    } finally {
      bridgeSessionCalls.delete(controller);
      if (bridgeSessionCalls.size === 0)
        activeCallsBySession.delete(event.sessionID);
    }
  }
  const registrations = [];
  try {
    if (context.session && context.shell) {
      const observeShell = (event) => {
        const description = event.tools.shell?.description ?? "";
        const match = /(?:^| )Commands run on (?:Windows|macOS|Linux) using ([a-zA-Z0-9_-]+)\./.exec(description);
        let agents = dialects.get(event.sessionID);
        if (!agents) {
          agents = new Map;
          dialects.set(event.sessionID, agents);
        }
        agents.set(event.agent, match?.[1] ? shellDialect(match[1]) : "unknown");
      };
      for (const name of ["context", "generate", "compaction"]) {
        registrations.push(await context.session.hook(name, observeShell));
      }
      registrations.push(await context.shell.hook("create.before", (event) => {
        const rewrite = rewrites.get(event.command);
        if (!rewrite)
          return;
        rewrites.delete(event.command);
        const actual = shellDialect(event.shell);
        if (actual !== rewrite.dialect) {
          event.command = rewrite.original;
          bridge.recordWrapping(rewrite.session, rewrite.call, "shell", "withdrawn", "dialect_mismatch", rewrite.dialect, actual);
        } else {
          bridge.recordWrapping(rewrite.session, rewrite.call, "shell", "adopted", null, rewrite.dialect, actual);
        }
      }));
    }
    registrations.push(await context.tool.hook("execute.before", handleBefore));
    registrations.push(await context.tool.hook("execute.after", handleResult));
  } catch (error) {
    for (const registration of registrations.reverse())
      await registration.dispose();
    bridge.close();
    throw error;
  }
  let eventLoop;
  const events = context.event;
  if (events) {
    eventLoop = (async () => {
      try {
        for await (const event of events.subscribe({ signal: eventController.signal })) {
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
    for (const sessionCalls of activeCallsBySession.values()) {
      for (const controller of sessionCalls)
        controller.abort();
    }
    bridge.close();
    dialects.clear();
    rewrites.clear();
    originalInputs.clear();
    for (const registration of registrations.reverse())
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
