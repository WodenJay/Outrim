// @bun
// adapters/pi.ts
import { spawn as nodeSpawn } from "child_process";

// adapters/calls.ts
class CallSnapshots {
  sessions = new Map;
  begin(session, call, input, cwd) {
    let calls = this.sessions.get(session);
    if (!calls) {
      calls = new Map;
      this.sessions.set(session, calls);
    }
    const snapshot = { session, call, input: structuredClone(input), cwd };
    calls.set(call, snapshot);
    return snapshot;
  }
  isCurrent(snapshot) {
    return this.sessions.get(snapshot.session)?.get(snapshot.call) === snapshot;
  }
  take(session, call) {
    const calls = this.sessions.get(session);
    const snapshot = calls?.get(call);
    calls?.delete(call);
    if (calls?.size === 0)
      this.sessions.delete(session);
    return snapshot === null ? { selection: "off" } : snapshot;
  }
  invalidate(session) {
    const sessions = session === undefined ? this.sessions.values() : [this.sessions.get(session)];
    for (const calls of sessions) {
      if (!calls)
        continue;
      for (const call of calls.keys())
        calls.set(call, null);
    }
  }
  clear() {
    this.sessions.clear();
  }
}

// adapters/reads.ts
import { realpathSync, statSync } from "fs";
import { isAbsolute, resolve } from "path";
function piBody(text, start, facts) {
  if (facts.firstLineExceedsLimit === true)
    return;
  const notice = /\n\n?\[(?:Showing lines (\d+)-(\d+) of \d+(?: \([^\r\n]+ limit\))?\. Use offset=(\d+) to continue\.|\d+ more lines in file\. Use offset=(\d+) to continue\.)\]$/.exec(text);
  if (facts.truncated === true && !notice)
    return;
  if (/\[(?:Showing lines |\d+ more lines in file\.)/.test(text) && !notice)
    return;
  const source = notice ? text.slice(0, notice.index) : text;
  const chunks = source.split(`
`);
  if (notice) {
    const end = start + chunks.length - 1;
    if (notice[1] && (Number(notice[1]) !== start || Number(notice[2]) !== end))
      return;
    if (Number(notice[3] ?? notice[4]) !== end + 1)
      return;
  }
  const lines = chunks.slice(0, -1).map((line) => ({ text: `${line}
`, rendered: `${line}
` }));
  const final_line = chunks.at(-1);
  if (!lines.length && !final_line)
    return;
  return {
    start,
    lines,
    ...final_line ? { final_line } : {},
    suffix: text.slice(lines.reduce((size, line) => size + line.rendered.length, 0))
  };
}
function openCodeBody(text, start, end, prefix, facts) {
  if (facts.linesTruncated === true)
    return;
  const rest = text.slice(prefix.length);
  const notice = /\n\[Output truncated\. Continue reading with offset: (\d+)\]$/.exec(rest);
  if (facts.truncated === true && !notice)
    return;
  if (notice && Number(notice[1]) !== end + 1)
    return;
  const source = notice ? rest.slice(0, notice.index) : rest;
  const chunks = source.split(`
`);
  if (chunks.length !== end - start + 1)
    return;
  const lines = [];
  for (let index = 0;index < chunks.length; index++) {
    const number = `${start + index}: `;
    const chunk = chunks[index];
    if (chunk === undefined || !chunk.startsWith(number))
      return;
    if (index < chunks.length - 1)
      lines.push({ text: `${chunk.slice(number.length)}
`, rendered: `${chunk}
` });
  }
  const final_line = chunks.at(-1);
  if (!final_line)
    return;
  return {
    start,
    prefix,
    lines,
    final_line,
    suffix: rest.slice(lines.reduce((size, line) => size + line.rendered.length, 0))
  };
}
function nativeReadSelection(host, input, text, cwd, facts = {}) {
  const path = host === "opencode" ? input.filePath ?? input.path : input.path;
  if (typeof path !== "string" || !isAbsolute(path) && !cwd)
    return;
  const start = input.offset ?? 1;
  const limit = input.limit;
  if (!Number.isSafeInteger(start) || start < 1 || limit !== undefined && (!Number.isSafeInteger(limit) || limit < 1))
    return;
  let label;
  let body;
  if (host === "opencode") {
    const range = /^Read file [^\r\n]+, lines (\d+)-(\d+)\n/.exec(text);
    if (!range || Number(range[1]) !== start || Number(range[2]) < start)
      return;
    label = `${range[1]}\u2013${range[2]}`;
    body = openCodeBody(text, start, Number(range[2]), range[0], facts);
  } else {
    if (!text)
      return;
    body = piBody(text, start, facts);
    const count = text.split(`
`).length - (text.endsWith(`
`) ? 1 : 0);
    const notice = /\[(?:Showing lines (\d+)-(\d+) of |\d+ more lines in file\. Use offset=(\d+) to continue\.)/.exec(text);
    const end = notice ? Number(notice[2] ?? Number(notice[3]) - 1) : start + count - 1;
    label = `${start}\u2013${end}`;
  }
  if ((facts.truncated === true || facts.linesTruncated === true) && !body)
    return;
  try {
    const absolute = realpathSync(resolve(cwd ?? "", path));
    if (!statSync(absolute).isFile())
      return;
    return {
      path: absolute,
      key: JSON.stringify([host, start, limit ?? null]),
      label,
      ...body ? { body } : {}
    };
  } catch {
    return;
  }
}

// adapters/shared.ts
import { readFileSync, realpathSync as realpathSync2, statSync as statSync2 } from "fs";
import { homedir } from "os";
import { isAbsolute as isAbsolute2, join, relative } from "path";

// adapters/bridge/transport.ts
function createTransport(executable, args, spawnProcess) {
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
  function receive(currentChild, chunk) {
    if (child !== currentChild)
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
        fail(currentChild);
        return;
      }
      if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
        fail(currentChild);
        return;
      }
      const response = parsed;
      const request = typeof response.id === "string" ? pending.get(response.id) : undefined;
      if (response.version !== 2 || !request) {
        fail(currentChild);
        return;
      }
      pending.delete(response.id);
      request.finish(response);
    }
  }
  function start() {
    let started;
    try {
      started = spawnProcess(executable, args, {
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
      ...payload.type === "process_parts" ? { request: { ...payload.request, id } } : {}
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
    exchange,
    close,
    get running() {
      return child !== undefined;
    }
  };
}

// adapters/shared.ts
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
  if (host === "opencode" && typeof details.linesTruncated === "boolean")
    facts.linesTruncated = details.linesTruncated;
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
    const root = realpathSync2(join(homedir(), ".outrim", "outputs"));
    return paths.length > 0 && paths.every((path) => {
      if (!isAbsolute2(path))
        return false;
      const resolved = realpathSync2(path);
      const within = relative(root, resolved);
      return within !== "" && !within.startsWith("..") && !isAbsolute2(within) && statSync2(resolved).isFile();
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
  const transport = createTransport(executable, ["bridge", "--recording", recording ? "on" : "off"], spawnProcess);
  const { exchange } = transport;
  let closed = false;
  let readEpoch = 0;
  const shellReads = new Set;
  const readIds = new WeakMap;
  function resetReads() {
    readEpoch++;
    shellReads.clear();
    if (transport.running)
      exchange({ type: "read_reset" });
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
  async function processParts(session, tool, texts, cwd, signal, selection, read) {
    if (read && texts.length === 1)
      return [
        await process(session, tool, texts[0] ?? "", signal, cwd, selection, { selection: read })
      ];
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
    const result = {
      text: response.text,
      disposition: response.disposition,
      appliedRule: response.applied_rule,
      originalPath: typeof response.original_path === "string" ? response.original_path : null,
      candidates: Array.isArray(response.candidates) ? response.candidates : [],
      reason: typeof response.reason === "string" ? response.reason : null
    };
    return result;
  }
  async function process(session, tool, text, signal, cwd, selection, read) {
    if (read) {
      if ("call" in read && !shellReads.delete(JSON.stringify([session, read.call])))
        return;
      const epoch = readEpoch;
      const facts = "selection" in read ? { type: "read_process", selection: read.selection } : { type: "shell_read", tool_call_id: read.call };
      const response = await exchange({
        version: 2,
        id: "",
        host,
        session,
        tool,
        text,
        ...cwd === undefined ? {} : { context: { cwd } },
        ...facts,
        policy: selection
      }, signal);
      if (epoch !== readEpoch)
        return;
      const result = processedResult(response);
      if (result && response)
        readIds.set(result, { id: response.id, epoch });
      return result;
    }
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
    if (rewritten && response?.read === true)
      shellReads.add(JSON.stringify([session, toolCallId]));
    recordWrapping(session, toolCallId, "shell", rewritten ? "planned" : "rejected", response?.reason ?? (response ? "unsupported_command" : "bridge_unavailable"), dialect);
    return rewritten;
  }
  function recordDelivery(original, outcome, signal) {
    if (closed || signal?.aborted)
      return;
    const texts = typeof original === "string" ? [original] : original;
    const beforeChars = texts.reduce((total, text) => total + Array.from(text).length, 0);
    const processed = typeof outcome === "object" ? outcome : undefined;
    const read = processed && readIds.get(processed);
    if (processed && read && read.epoch === readEpoch) {
      readIds.delete(processed);
      exchange({ type: "read_delivered", request_id: read.id }, signal);
    }
    exchange({
      type: "record_delivery",
      disposition: processed?.disposition ?? (typeof outcome === "string" ? outcome : "failed_open"),
      applied_rule: processed?.appliedRule ?? null,
      before_chars: beforeChars,
      after_chars: processed ? Array.from(processed.text).length : beforeChars
    });
  }
  function recordObservation(context, original, final, facts) {
    if (closed || !recording)
      return;
    exchange({
      type: "record_observation",
      host,
      session: context.session,
      tool_call_id: context.toolCallId,
      tool: facts.metadataOnly ? { ...context.tool, input: null } : context.tool,
      original_text: facts.metadataOnly ? null : original,
      final_text: facts.metadataOnly ? null : final,
      metadata_only: facts.metadataOnly ?? false,
      diagnostics: {
        adapter_build: "18dde03cc46fecef32b86a304ec0ad0fc3ecbfeb4bbc125cfea2e23e8be97a01",
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
    });
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
    transport.close();
  }
  return {
    rewrite,
    selectPolicy,
    process,
    processParts,
    resetReads,
    recordDelivery,
    recordObservation,
    recordRecovery,
    recordWrapping,
    close
  };
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
function isShell2(name) {
  return ["bash", "powershell", "shell"].includes(name);
}
async function shellDialect(pi, name) {
  try {
    const tool = pi.getAllTools?.().find((tool) => tool.name === name);
    if (name !== "bash" || tool?.sourceInfo.source !== "builtin" || tool.sourceInfo.path !== "builtin:bash")
      return "unknown";
    const settings = pi.getSettings?.();
    if (!settings || settings.shellCommandPrefix)
      return "unknown";
    const packageName = "@earendil-works/pi-coding-agent";
    const sdk = await import(packageName);
    const path = sdk.SettingsManager.inMemory(settings).getShellPath();
    const shell = sdk.getShellConfig(path).shell;
    return /(?:^|[/\\\\])bash(?:\.exe)?$/i.test(shell) ? "bash" : "unknown";
  } catch {
    return "unknown";
  }
}
var defaultSpawn = (executable, args, options) => nodeSpawn(executable, args, options);
function registerPiExtension(pi, options = {}) {
  const bridge = createBridge("pi", options.executable ?? BRIDGE_EXECUTABLE, options.spawnProcess ?? defaultSpawn);
  const calls = new CallSnapshots;
  let closed = false;
  pi.on("tool_call", async (event, context) => {
    if (closed)
      return;
    const session = context.sessionManager.getSessionId();
    const snapshot = event.parentToolCallId ? undefined : calls.begin(session, event.toolCallId, event.input, context.cwd);
    if (!isShell2(event.toolName) && snapshot) {
      snapshot.selection = "off";
      snapshot.selection = await bridge.selectPolicy(session, {
        name: event.toolName,
        input: snapshot.input,
        source: sourceFor("pi", event.toolName, snapshot.input),
        exit_code: null
      }, snapshot.cwd, context.signal);
      if (!calls.isCurrent(snapshot))
        return;
    }
    const wrapping = (state, reason, dialect = "unknown") => bridge.recordWrapping(session, event.toolCallId, event.toolName, state, reason, dialect);
    if (!isShell2(event.toolName) || event.parentToolCallId || typeof event.input.command !== "string") {
      wrapping("not_attempted", event.parentToolCallId ? "nested_call" : "source_not_applicable");
      return;
    }
    const original = event.input.command;
    try {
      const dialect = options.dialect ?? await shellDialect(pi, event.toolName);
      if (!snapshot || !calls.isCurrent(snapshot))
        return;
      if (dialect === "unknown") {
        wrapping("not_attempted", "unknown_dialect");
        return;
      }
      const command = await bridge.rewrite(session, event.toolCallId, original, dialect, context.signal, {
        name: event.toolName,
        input: snapshot.input,
        source: sourceFor("pi", event.toolName, event.input),
        exit_code: null
      });
      if (!calls.isCurrent(snapshot))
        return;
      if (command !== undefined && !context.signal?.aborted && event.input.command === original) {
        event.input.command = command;
        wrapping("adopted", null, dialect);
      } else if (command !== undefined) {
        wrapping("withdrawn", context.signal?.aborted ? "cancelled" : "changed_input", dialect);
      }
    } catch {}
  });
  pi.on("tool_result", async (event, context) => {
    if (closed)
      return;
    if (event.toolName === "compress")
      bridge.resetReads();
    const snapshot = event.parentToolCallId ? undefined : calls.take(context.sessionManager.getSessionId(), event.toolCallId);
    const originalInput = snapshot?.input ?? event.input;
    const cwd = snapshot?.cwd ?? context.cwd;
    const selection = snapshot?.selection;
    const attribution = recoveryAttribution(event.toolName, originalInput);
    if (attribution && !event.isError && (!isShell2(event.toolName) || (exitCodeFrom(event.structuredContent) ?? 0) === 0) && !context.signal?.aborted && !event.parentToolCallId) {
      bridge.recordRecovery(context.sessionManager.getSessionId(), event.toolCallId, event.content.flatMap((part) => part.type === "text" && typeof part.text === "string" ? [part.text] : []), attribution);
    }
    if (isShell2(event.toolName)) {
      const text = event.content.flatMap((part) => part.type === "text" && typeof part.text === "string" ? [part.text] : []).join(`
`);
      bridge.recordObservation({
        session: context.sessionManager.getSessionId(),
        toolCallId: event.toolCallId,
        diagnostics: hostOutputFacts("pi", event.details),
        tool: {
          name: event.toolName,
          input: null,
          source: "unknown",
          exit_code: exitCodeFrom(event.structuredContent)
        }
      }, text, null, {
        outcome: context.signal?.aborted ? "cancelled" : "unsupported",
        reason: event.isError ? "host_error" : "shell_result_observed",
        metadataOnly: true
      });
      const [content] = event.content;
      if (event.isError || hostOutputFacts("pi", event.details).completeness === "host_truncated" || event.parentToolCallId || event.content.length !== 1 || content?.type !== "text" || typeof content.text !== "string" || inputReferencesOriginals(originalInput))
        return;
      const processed = await bridge.process(context.sessionManager.getSessionId(), {
        name: event.toolName,
        input: originalInput,
        source: "unknown",
        exit_code: exitCodeFrom(event.structuredContent)
      }, content.text, context.signal, cwd, selection, { call: event.toolCallId });
      if (!processed || context.signal?.aborted || processed.reason === "unverified_shell_read")
        return;
      bridge.recordDelivery(content.text, processed, context.signal);
      if (processed.text === content.text)
        return;
      return {
        content: [{ ...content, type: "text", text: processed.text }],
        details: event.details,
        structuredContent: event.structuredContent,
        isError: event.isError,
        usage: event.usage
      };
    }
    const observation = {
      session: context.sessionManager.getSessionId(),
      toolCallId: event.toolCallId ?? null,
      diagnostics: hostOutputFacts("pi", event.details),
      tool: {
        name: event.toolName,
        input: originalInput,
        source: sourceFor("pi", event.toolName, originalInput),
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
    if (inputReferencesOriginals(originalInput)) {
      bridge.recordObservation(observation, text, text, {
        outcome: context.signal?.aborted ? "cancelled" : "recovery_skipped",
        reason: "original_directory_in_arguments"
      });
      bridge.recordDelivery(text, "recovery_skipped", context.signal);
      return;
    }
    const processed = await bridge.process(observation.session, observation.tool, text, context.signal, cwd, selection, event.toolName === "read" && !event.isError ? (() => {
      const selection = nativeReadSelection("pi", originalInput, text, cwd, observation.diagnostics.host_truncation ?? {});
      return selection ? { selection } : undefined;
    })() : undefined);
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
  for (const event of ["session_start", "session_compact", "session_tree"]) {
    pi.on(event, () => {
      calls.invalidate();
      bridge.resetReads();
    });
  }
  pi.on("session_shutdown", () => {
    closed = true;
    calls.clear();
    bridge.close();
  });
}
function piExtension(pi) {
  registerPiExtension(pi);
}
export {
  OUTRIM_EXTENSION_MARKER,
  piExtension as default,
  registerPiExtension
};
