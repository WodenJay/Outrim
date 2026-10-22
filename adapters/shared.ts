import { readFileSync, realpathSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { isAbsolute, join, relative } from "node:path";

export type Host = "pi" | "opencode";
export type WrappingState = "not_attempted" | "rejected" | "planned" | "adopted" | "withdrawn";

interface RecoveryEvidence {
  attribution: "confirmed" | "unattributed";
  original_paths: string[];
}
export type Source =
  | "source_code"
  | "configuration"
  | "search"
  | "build_log"
  | "test_log"
  | "runtime_log"
  | "unknown";

export interface AbortSignalLike {
  readonly aborted: boolean;
  addEventListener(type: "abort", listener: () => void, options?: { once?: boolean }): void;
  removeEventListener(type: "abort", listener: () => void): void;
}

export interface ToolInput {
  name: string;
  input: unknown;
  source: Source;
  exit_code: number | null;
}

const TEST_COMMANDS = [
  /^\s*cargo\s+test(?=\s|$)/i,
  /^\s*(?:bun|npm|pnpm|yarn)\s+(?:test|run\s+test)(?=\s|$)/i,
  /^\s*(?:pytest|go\s+test|jest|vitest)(?=\s|$)/i,
];

const BUILD_COMMANDS = [
  /^\s*cargo\s+(?:build|check|clippy)(?=\s|$)/i,
  /^\s*(?:bun|npm|pnpm|yarn)\s+(?:build|run\s+build)(?=\s|$)/i,
  /^\s*(?:go\s+build|tsc|vite\s+build|cmake\s+--build|make)(?=\s|$)/i,
];

const RUNTIME_COMMANDS = [
  /^\s*cargo\s+run(?=\s|$)/i,
  /^\s*(?:bun|npm|pnpm|yarn)\s+(?:start|run\s+(?:dev|start))(?=\s|$)/i,
  /^\s*gh\s+run\s+watch(?=\s|$)/i,
];

function matchesAny(patterns: RegExp[], command: string): boolean {
  return patterns.some((pattern) => pattern.test(command));
}

function isShell(host: Host, toolName: string): boolean {
  return (
    toolName === "bash" ||
    toolName === "powershell" ||
    (host === "opencode" && toolName === "shell")
  );
}

// Recognize only a bounded simple-command grammar, not a full shell language.
// Generic entry points use only a common subset: their dialect is unknown,
// and cmd.exe does not share Bash/PowerShell single-quote or expansion rules.
function isSimpleCommand(command: string, dialect: "bash" | "powershell" | "generic"): boolean {
  const head = /^[ \t]*([A-Za-z0-9_./:-]+)(?=[ \t]|$)/.exec(command)?.[1]?.toLowerCase();
  if (
    !head ||
    /(?:^|\/)(?:bash|sh|zsh|dash|fish|ksh|csh|tcsh|cmd|pwsh|powershell)(?:\.exe)?$/.test(head) ||
    [
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
      "coproc",
    ].includes(head)
  )
    return false;
  // PowerShell also treats typographic quotes as delimiters. They are outside
  // this recognizer's supported quoting grammar, even inside ASCII quotes.
  if (/[\0\u2018\u2019\u201c\u201d]/.test(command)) return false;
  if (dialect === "generic" && /[%!^\r\n]/.test(command)) return false;
  let quote: "'" | '"' | undefined;
  for (const char of command) {
    if (quote === "'") {
      if (char === "'") quote = undefined;
    } else if ((dialect !== "powershell" && char === "\\") || char === "`" || char === "$") {
      return false;
    } else if (quote === '"') {
      if (char === '"') quote = undefined;
    } else if (char === "'" || char === '"') {
      if (dialect === "generic" && char === "'") return false;
      quote = char;
    } else if (
      "|&;<>()[]{}#@,!%^\r\n\0".includes(char) ||
      (/\s/.test(char) && char !== " " && char !== "\t")
    ) {
      return false;
    }
  }
  return quote === undefined;
}

export function shellSkipReason(
  host: Host,
  toolName: string,
  input: Record<string, unknown>,
): string | null {
  if (!isShell(host, toolName)) return null;
  const command = input.command;
  if (
    typeof command !== "string" ||
    !isSimpleCommand(
      command,
      toolName === "bash" || toolName === "powershell" ? toolName : "generic",
    )
  ) {
    return "compound_or_uncertain_shell_command";
  }
  return null;
}

export function sourceFor(host: Host, toolName: string, input: Record<string, unknown>): Source {
  if (
    ["find", "grep", "ls"].includes(toolName) ||
    (host === "opencode" && ["glob", "list"].includes(toolName))
  )
    return "search";
  if (!isShell(host, toolName) || shellSkipReason(host, toolName, input)) return "unknown";
  const command = input.command;
  if (typeof command !== "string") return "unknown";
  if (/^[ \t]*(?:rg|grep|git[ \t]+grep)(?=[ \t]|$)/.test(command)) return "search";
  if (matchesAny(TEST_COMMANDS, command)) return "test_log";
  if (matchesAny(BUILD_COMMANDS, command)) return "build_log";
  if (matchesAny(RUNTIME_COMMANDS, command)) return "runtime_log";
  return "unknown";
}

export type PolicySelection = "builtin" | "off" | { methods: string[] };

interface CoreRequest {
  version: 2;
  id: string;
  host: Host;
  session: string;
  tool: ToolInput;
  text: string;
  context?: { cwd: string; command?: string[] };
}

type Disposition =
  | "compressed"
  | "protected"
  | "recovery_skipped"
  | "not_shorter"
  | "no_match"
  | "failed_open";

export type ObservationOutcome = Disposition | "unsupported" | "cancelled" | "mixed";

export interface Candidate {
  rule: string;
  body_chars: number;
  final_chars: number | null;
  reason: string;
}

interface CoreResponse {
  selection?: PolicySelection;
  parts?: CoreResponse[];
  read?: boolean;
  version: number;
  id: string;
  text?: string;
  command?: string | null;
  disposition?: Disposition;
  applied_rule?: string | null;
  original_path?: string | null;
  candidates?: Candidate[];
  reason?: string | null;
}

export interface ProcessedResult {
  text: string;
  disposition: Disposition;
  appliedRule: string | null;
  originalPath: string | null;
  candidates: Candidate[];
  reason: string | null;
}

export interface ObservationContext {
  diagnostics?: Record<string, unknown>;
  session: string | null;
  toolCallId: string | null;
  tool: ToolInput;
}

// Retain only verified host scalar facts, never truncator content or output paths.
export function hostOutputFacts(host: Host, value: unknown): Record<string, unknown> {
  const details = value && typeof value === "object" ? (value as Record<string, unknown>) : {};
  const raw = host === "pi" ? details.truncation : details;
  const truncation = raw && typeof raw === "object" ? (raw as Record<string, unknown>) : {};
  const facts: Record<string, unknown> = {};
  if (typeof truncation.truncated === "boolean") facts.truncated = truncation.truncated;
  if (host === "pi") {
    for (const name of [
      "totalLines",
      "totalBytes",
      "outputLines",
      "outputBytes",
      "maxLines",
      "maxBytes",
    ]) {
      const number = truncation[name];
      if (typeof number === "number" && Number.isSafeInteger(number) && number >= 0)
        facts[name] = number;
    }
    if (truncation.truncatedBy === "bytes" || truncation.truncatedBy === "lines")
      facts.truncatedBy = truncation.truncatedBy;
    for (const name of ["lastLinePartial", "firstLineExceedsLimit"]) {
      if (typeof truncation[name] === "boolean") facts[name] = truncation[name];
    }
    for (const name of ["matchLimitReached", "resultLimitReached", "entryLimitReached"]) {
      const number = details[name];
      if (typeof number === "number" && Number.isSafeInteger(number) && number > 0)
        facts[name] = number;
    }
    if (typeof details.linesTruncated === "boolean") facts.linesTruncated = details.linesTruncated;
  }
  const limited =
    facts.truncated === true ||
    facts.linesTruncated === true ||
    ["matchLimitReached", "resultLimitReached", "entryLimitReached"].some((name) => name in facts);
  return {
    completeness: limited ? "host_truncated" : facts.truncated === false ? "complete" : "unknown",
    // These sizes describe the host truncator's input/output, not a whole file.
    host_truncation: Object.keys(facts).length ? facts : null,
  };
}

export interface ObservationFacts {
  metadataOnly?: boolean;
  diagnostics?: Record<string, unknown>;
  outcome: ObservationOutcome;
  appliedRule?: string | null;
  originalPath?: string | null;
  candidates?: Candidate[];
  reason?: string | null;
  parts?: ProcessingPart[];
}

export interface ProcessingPart {
  outcome: Disposition;
  applied_rule: string | null;
  original_path: string | null;
  candidates: Candidate[];
  reason: string | null;
}

export interface ProcessingFacts {
  parts: ProcessingPart[];
  outcome: ObservationOutcome;
  appliedRule: string | null;
  originalPath: string | null;
  candidates: Candidate[];
  reason: string | null;
}

function distinct(values: Array<string | null>): string[] {
  return [...new Set(values.flatMap((value) => (value === null ? [] : [value])))];
}

/** Collapses the per-text responses of one tool result into one observation. */
export function processingFacts(
  results: ReadonlyArray<ProcessedResult | undefined>,
): ProcessingFacts {
  const processed = results.filter((result): result is ProcessedResult => result !== undefined);
  const dispositions = new Set(processed.map((result) => result.disposition));
  const rules = distinct(processed.map((result) => result.appliedRule));
  const paths = distinct(processed.map((result) => result.originalPath));
  const reasons = distinct(processed.map((result) => result.reason));

  if (processed.length !== results.length) dispositions.add("failed_open");
  const outcome =
    dispositions.size > 1 ? "mixed" : (dispositions.values().next().value ?? "failed_open");

  return {
    parts: results.map((result) => ({
      outcome: result?.disposition ?? "failed_open",
      applied_rule: result?.appliedRule ?? null,
      original_path: result?.originalPath ?? null,
      candidates: result?.candidates ?? [],
      reason: result?.reason ?? (result ? null : "bridge_unavailable"),
    })),
    outcome,
    appliedRule: rules.length === 1 ? (rules[0] ?? null) : null,
    originalPath: paths.length === 1 ? (paths[0] ?? null) : null,
    candidates: processed.flatMap((result) => result.candidates),
    reason: reasons.length === 0 ? null : reasons.join("; "),
  };
}

export interface BridgeChild {
  stdin: {
    write(chunk: string, callback: (error?: Error | null) => void): boolean;
    end(): void;
    on(event: "error", handler: (error: Error) => void): unknown;
  };
  stdout: {
    setEncoding(encoding: "utf8"): void;
    on(event: "data", handler: (chunk: string) => void): unknown;
  };
  on(event: "error", handler: (error: Error) => void): unknown;
  on(event: "exit", handler: (code: number | null, signal: string | null) => void): unknown;
  kill(): boolean;
}

export type SpawnProcess = (
  executable: string,
  args: string[],
  options: { stdio: ["pipe", "pipe", "ignore"]; windowsHide: true },
) => BridgeChild;

export function inputReferencesOriginals(value: unknown): boolean {
  if (typeof value === "string") {
    const normalized = value.replaceAll("\\", "/");
    return /(?:^|[/'"`\s=:([{])\.outrim\/outputs(?:\/|$|[/'"`\s),\]}])/i.test(normalized);
  }
  if (Array.isArray(value)) return value.some(inputReferencesOriginals);
  if (value !== null && typeof value === "object") {
    return Object.values(value).some(inputReferencesOriginals);
  }
  return false;
}

// Stricter than recovery protection: only literal, successful original reads
// justify a deduction. Never read file contents to estimate returned characters.
export function recoveryAttribution(tool: string, input: unknown): RecoveryEvidence | null {
  if (!inputReferencesOriginals(input)) return null;
  const unattributed: RecoveryEvidence = { attribution: "unattributed", original_paths: [] };
  if (!input || typeof input !== "object" || Array.isArray(input)) return unattributed;
  const args = input as Record<string, unknown>;
  let paths: string[];
  if (tool === "read") {
    const path = args.path ?? args.filePath ?? args.file_path;
    if (typeof path !== "string") return unattributed;
    paths = [path];
  } else if (["bash", "powershell", "shell"].includes(tool) && typeof args.command === "string") {
    // No expansions, redirections, scripts, transformations or compound commands.
    const command = args.command;
    if (/[\\`$\r\n\0|&;<>()*?{}[\]]/.test(command.replaceAll("\\", "/"))) return unattributed;
    const words: string[] = [];
    const pattern = /\s*(?:'([^']*)'|"([^"]*)"|([^\s'"\\]+))(?=\s|$)/gy;
    let end = 0;
    for (const match of command.matchAll(pattern)) {
      if (match.index !== end) return unattributed;
      words.push(match[1] ?? match[2] ?? match[3] ?? "");
      end = match.index + match[0].length;
    }
    if (command.slice(end).trim()) return unattributed;
    const program = words.shift();
    if (program !== "cat" && program !== "head") return unattributed;
    if (program === "head" && words[0] === "-n" && /^\d+$/.test(words[1] ?? "")) words.splice(0, 2);
    if (program === "head" && /^-\d+$/.test(words[0] ?? "")) words.shift();
    if (words[0] === "--") words.shift();
    if (words.some((word) => word.startsWith("-"))) return unattributed;
    paths = words;
  } else return unattributed;
  try {
    const root = realpathSync(join(homedir(), ".outrim", "outputs"));
    return paths.length > 0 &&
      paths.every((path) => {
        if (!isAbsolute(path)) return false;
        const resolved = realpathSync(path);
        const within = relative(root, resolved);
        return (
          within !== "" &&
          !within.startsWith("..") &&
          !isAbsolute(within) &&
          statSync(resolved).isFile()
        );
      })
      ? { attribution: "confirmed", original_paths: [...new Set(paths)] }
      : unattributed;
  } catch {
    return unattributed;
  }
}

export function createBridge(
  host: Host,
  executable: string,
  spawnProcess: SpawnProcess,
): {
  rewrite(
    session: string,
    toolCallId: string,
    command: string,
    dialect: "bash" | "powershell" | "unknown",
    signal?: AbortSignalLike,
    tool?: ToolInput,
  ): Promise<string | undefined>;
  selectPolicy(
    session: string,
    tool: ToolInput,
    cwd?: string,
    signal?: AbortSignalLike,
  ): Promise<PolicySelection>;
  processParts(
    session: string,
    tool: ToolInput,
    texts: string[],
    cwd: string | undefined,
    signal?: AbortSignalLike,
    selection?: PolicySelection,
    read?: import("./reads").ReadSelection,
  ): Promise<Array<ProcessedResult | undefined>>;
  process(
    session: string,
    tool: ToolInput,
    text: string,
    signal?: AbortSignalLike,
    cwd?: string,
    selection?: PolicySelection,
    read?: { selection: import("./reads").ReadSelection } | { call: string },
  ): Promise<ProcessedResult | undefined>;
  resetReads(): void;
  recordDelivery(
    original: string | readonly string[],
    outcome: ProcessedResult | "recovery_skipped" | "protected" | undefined,
    signal?: AbortSignalLike,
  ): void;
  recordObservation(
    context: ObservationContext,
    original: string | null,
    final: string | null,
    facts: ObservationFacts,
  ): void;
  recordRecovery(
    session: string,
    toolCallId: string,
    texts: readonly string[],
    evidence: RecoveryEvidence,
  ): void;
  recordWrapping(
    session: string,
    call: string,
    tool: string,
    state: WrappingState,
    reason: string | null,
    dialect: string,
    actual?: string,
  ): void;
  close(): void;
} {
  // Snapshot once when the host loads, including across bridge restarts.
  let recording = false;
  try {
    recording =
      JSON.parse(readFileSync(join(homedir(), ".outrim", "recording.json"), "utf8")).recording ===
      true;
  } catch {
    // Missing or unreadable settings fail closed for content retention.
  }
  let child: BridgeChild | undefined;
  let nextId = 0;
  let buffer = "";
  let closed = false;
  const pending = new Map<string, { finish(response: CoreResponse | undefined): void }>();

  function fail(failedChild: BridgeChild): void {
    if (child !== failedChild) return;
    child = undefined;
    buffer = "";
    for (const request of pending.values()) request.finish(undefined);
    pending.clear();
    failedChild.kill();
  }

  function receive(failedChild: BridgeChild, chunk: string): void {
    if (child !== failedChild) return;
    buffer += chunk;
    while (true) {
      const newline = buffer.indexOf("\n");
      if (newline < 0) return;
      const line = buffer.slice(0, newline);
      buffer = buffer.slice(newline + 1);
      let parsed: unknown;
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
      const response = parsed as CoreResponse;
      const request = typeof response.id === "string" ? pending.get(response.id) : undefined;
      if (response.version !== 2 || !request) {
        fail(failedChild);
        return;
      }
      pending.delete(response.id);
      request.finish(response);
    }
  }

  function start(): BridgeChild | undefined {
    let started: BridgeChild;
    try {
      started = spawnProcess(executable, ["bridge", "--recording", recording ? "on" : "off"], {
        stdio: ["pipe", "pipe", "ignore"],
        windowsHide: true,
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

  function exchange(
    payload: Record<string, unknown>,
    signal?: AbortSignalLike,
    timeoutMs?: number,
  ): Promise<CoreResponse | undefined> {
    if (closed || signal?.aborted) return Promise.resolve(undefined);
    const currentChild = child ?? start();
    if (!currentChild) return Promise.resolve(undefined);

    const id = String(++nextId);
    const request = {
      ...payload,
      version: 2,
      id,
      ...(payload.type === "process_parts"
        ? {
            request: { ...(payload.request as CoreRequest), id },
          }
        : {}),
    };
    let line: string;
    try {
      line = `${JSON.stringify(request)}\n`;
    } catch {
      return Promise.resolve(undefined);
    }

    return new Promise((resolve) => {
      let settled = false;
      let timer: ReturnType<typeof setTimeout> | undefined;
      const finish = (response: CoreResponse | undefined) => {
        if (settled) return;
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
      if (timeoutMs !== undefined) timer = setTimeout(() => fail(currentChild), timeoutMs);
      try {
        currentChild.stdin.write(line, (error) => {
          if (error) fail(currentChild);
        });
      } catch {
        fail(currentChild);
      }
    });
  }

  let readEpoch = 0;
  const shellReads = new Set<string>();
  const readIds = new WeakMap<ProcessedResult, { id: string; epoch: number }>();
  function resetReads(): void {
    readEpoch++;
    shellReads.clear();
    if (child) void exchange({ type: "read_reset" });
  }

  async function selectPolicy(
    session: string,
    tool: ToolInput,
    cwd?: string,
    signal?: AbortSignalLike,
  ): Promise<PolicySelection> {
    try {
      const request: CoreRequest = {
        version: 2,
        id: "",
        host,
        session,
        tool,
        text: "",
        ...(cwd === undefined ? {} : { context: { cwd } }),
      };
      const response = await exchange({ type: "resolve_policy", request }, signal, 5_000);
      return response?.selection ?? "off";
    } catch {
      return "off";
    }
  }

  async function processParts(
    session: string,
    tool: ToolInput,
    texts: string[],
    cwd: string | undefined,
    signal?: AbortSignalLike,
    selection?: PolicySelection,
    read?: import("./reads").ReadSelection,
  ): Promise<Array<ProcessedResult | undefined>> {
    if (read && texts.length === 1)
      return [
        await process(session, tool, texts[0] ?? "", signal, cwd, selection, { selection: read }),
      ];
    const request: CoreRequest = {
      version: 2,
      id: "",
      host,
      session,
      tool,
      text: "",
      ...(cwd === undefined ? {} : { context: { cwd } }),
    };
    const response = await exchange({ type: "process_parts", request, texts, selection }, signal);
    if (!Array.isArray(response?.parts) || response.parts.length !== texts.length)
      return texts.map(() => undefined);
    return response.parts.map(processedResult);
  }

  function processedResult(response: CoreResponse | undefined): ProcessedResult | undefined {
    if (
      !response ||
      typeof response.text !== "string" ||
      typeof response.disposition !== "string" ||
      !(response.applied_rule === null || typeof response.applied_rule === "string")
    )
      return undefined;
    const result: ProcessedResult = {
      text: response.text,
      disposition: response.disposition as Disposition,
      appliedRule: response.applied_rule,
      originalPath: typeof response.original_path === "string" ? response.original_path : null,
      candidates: Array.isArray(response.candidates) ? response.candidates : [],
      reason: typeof response.reason === "string" ? response.reason : null,
    };
    return result;
  }

  async function process(
    session: string,
    tool: ToolInput,
    text: string,
    signal?: AbortSignalLike,
    cwd?: string,
    selection?: PolicySelection,
    read?: { selection: import("./reads").ReadSelection } | { call: string },
  ): Promise<ProcessedResult | undefined> {
    if (read) {
      if ("call" in read && !shellReads.delete(JSON.stringify([session, read.call])))
        return undefined;
      const epoch = readEpoch;
      const facts =
        "selection" in read
          ? { type: "read_process", selection: read.selection }
          : { type: "shell_read", tool_call_id: read.call };
      const response = await exchange(
        {
          version: 2,
          id: "",
          host,
          session,
          tool,
          text,
          ...(cwd === undefined ? {} : { context: { cwd } }),
          ...facts,
          policy: selection,
        },
        signal,
      );
      if (epoch !== readEpoch) return undefined;
      const result = processedResult(response);
      if (result && response) readIds.set(result, { id: response.id, epoch });
      return result;
    }
    if (selection !== undefined)
      return (await processParts(session, tool, [text], cwd, signal, selection))[0];
    const payload: CoreRequest = {
      version: 2,
      id: "",
      host,
      session,
      tool,
      text,
      ...(cwd === undefined ? {} : { context: { cwd } }),
    };
    return processedResult(await exchange({ ...payload }, signal));
  }

  async function rewrite(
    session: string,
    toolCallId: string,
    command: string,
    dialect: "bash" | "powershell" | "unknown",
    signal?: AbortSignalLike,
    tool?: ToolInput,
  ): Promise<string | undefined> {
    const response = await exchange(
      {
        type: "rewrite",
        tool,
        host,
        session,
        tool_call_id: toolCallId,
        unit_id: `${toolCallId}:0`,
        recording,
        dialect,
        command,
      },
      signal,
      5_000,
    );
    const rewritten = typeof response?.command === "string" ? response.command : undefined;
    if (rewritten && response?.read === true) shellReads.add(JSON.stringify([session, toolCallId]));
    recordWrapping(
      session,
      toolCallId,
      "shell",
      rewritten ? "planned" : "rejected",
      response?.reason ?? (response ? "unsupported_command" : "bridge_unavailable"),
      dialect,
    );
    return rewritten;
  }

  // Called only when the host adopts the result. An absent outcome records fail-open;
  // recovery may cover several text parts, but remains one unchanged delivery.
  function recordDelivery(
    original: string | readonly string[],
    outcome: ProcessedResult | "recovery_skipped" | "protected" | undefined,
    signal?: AbortSignalLike,
  ): void {
    if (closed || signal?.aborted) return;
    const currentChild = child ?? start();
    if (!currentChild) return;

    const texts = typeof original === "string" ? [original] : original;
    const beforeChars = texts.reduce((total, text) => total + Array.from(text).length, 0);
    const processed = typeof outcome === "object" ? outcome : undefined;
    const read = processed && readIds.get(processed);
    if (processed && read && read.epoch === readEpoch) {
      readIds.delete(processed);
      void exchange({ type: "read_delivered", request_id: read.id }, signal);
    }
    const id = String(++nextId);
    const request = {
      type: "record_delivery",
      version: 2,
      id,
      disposition:
        processed?.disposition ?? (typeof outcome === "string" ? outcome : "failed_open"),
      applied_rule: processed?.appliedRule ?? null,
      before_chars: beforeChars,
      after_chars: processed ? Array.from(processed.text).length : beforeChars,
    };
    let line: string;
    try {
      line = `${JSON.stringify(request)}\n`;
    } catch {
      return;
    }

    pending.set(id, { finish: () => {} });
    try {
      currentChild.stdin.write(line, (error) => {
        if (error) fail(currentChild);
      });
    } catch {
      fail(currentChild);
    }
  }

  // Observations record the result itself, so an aborted signal is only an
  // outcome fact (cancelled), never a reason to drop the record.
  function recordObservation(
    context: ObservationContext,
    original: string | null,
    final: string | null,
    facts: ObservationFacts,
  ): void {
    if (closed || !recording) return;
    const currentChild = child ?? start();
    if (!currentChild) return;

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
        adapter_build: "__OUTRIM_ADAPTER_BUILD__",
        output_origin: "host_result",
        completeness: "unknown",
        observed_chars: original === null ? null : Array.from(original).length,
        observed_bytes: original === null ? null : Buffer.byteLength(original, "utf8"),
        ...context.diagnostics,
        ...facts.diagnostics,
      },
      outcome: facts.outcome,
      applied_rule: facts.appliedRule ?? null,
      original_path: facts.originalPath ?? null,
      candidates: facts.candidates ?? [],
      reason: facts.reason ?? null,
      parts: facts.parts ?? [],
    };
    let line: string;
    try {
      line = `${JSON.stringify(request)}\n`;
    } catch {
      return;
    }

    pending.set(id, { finish: () => {} });
    try {
      currentChild.stdin.write(line, (error) => {
        if (error) fail(currentChild);
      });
    } catch {
      fail(currentChild);
    }
  }

  function recordRecovery(
    session: string,
    toolCallId: string,
    texts: readonly string[],
    evidence: RecoveryEvidence,
  ): void {
    // The durable identity is independent of bridge transport/restarts.
    void exchange({
      type: "record_recovery",
      host,
      session,
      tool_call_id: toolCallId,
      event_id: JSON.stringify([host, session, toolCallId, "recovery"]),
      characters: texts.reduce((count, text) => count + Array.from(text).length, 0),
      ...evidence,
    });
  }

  function recordWrapping(
    session: string,
    call: string,
    tool: string,
    state: WrappingState,
    reason: string | null,
    dialect: string,
    actual?: string,
  ): void {
    recordObservation(
      {
        session,
        toolCallId: call,
        tool: { name: tool, input: null, source: "unknown", exit_code: null },
      },
      null,
      null,
      {
        outcome: "unsupported",
        metadataOnly: true,
        reason,
        diagnostics: {
          wrapping: state,
          attempt_id: JSON.stringify([host, session, call]),
          requested_dialect: dialect,
          actual_dialect: actual ?? null,
          output_origin: null,
        },
      },
    );
  }

  function close(): void {
    if (closed) return;
    closed = true;
    const closingChild = child;
    child = undefined;
    buffer = "";
    for (const request of pending.values()) request.finish(undefined);
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
    resetReads,
    recordDelivery,
    recordObservation,
    recordRecovery,
    recordWrapping,
    close,
  };
}
