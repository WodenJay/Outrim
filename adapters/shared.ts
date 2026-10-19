import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

export type Host = "pi" | "opencode";
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
  /^\s*cargo\s+test\b/i,
  /^\s*(?:bun\s+(?:test|run\s+test)|npm\s+(?:test|run\s+test)|pnpm\s+(?:test|run\s+test)|yarn\s+(?:test|run\s+test))\b/i,
  /^\s*(?:pytest|go\s+test|jest|vitest)\b/i,
];

const BUILD_COMMANDS = [
  /^\s*cargo\s+(?:build|check|clippy)\b/i,
  /^\s*(?:bun|npm|pnpm|yarn)\s+(?:build|run\s+build)\b/i,
  /^\s*(?:go\s+build|tsc|vite\s+build|cmake\s+--build|make)\b/i,
];

const RUNTIME_COMMANDS = [
  /^\s*cargo\s+run\b/i,
  /^\s*(?:bun|npm|pnpm|yarn)\s+(?:start|run\s+(?:dev|start))\b/i,
  /^\s*(?:node|python(?:3(?:\.\d+)?)?|go)\s+(?:\S+|$)/i,
];

function matchesAny(patterns: RegExp[], command: string): boolean {
  return patterns.some((pattern) => pattern.test(command));
}

export function sourceFor(host: Host, toolName: string, input: Record<string, unknown>): Source {
  if (
    ["find", "grep", "ls"].includes(toolName) ||
    (host === "opencode" && ["glob", "list"].includes(toolName))
  )
    return "search";
  if (
    toolName !== "bash" &&
    toolName !== "powershell" &&
    !(host === "opencode" && toolName === "shell")
  )
    return "unknown";
  const command = input.command;
  if (typeof command !== "string") return "unknown";
  if (matchesAny(TEST_COMMANDS, command)) return "test_log";
  if (matchesAny(BUILD_COMMANDS, command)) return "build_log";
  if (matchesAny(RUNTIME_COMMANDS, command)) return "runtime_log";
  return "unknown";
}

interface CoreRequest {
  version: 2;
  id: string;
  host: Host;
  session: string;
  tool: ToolInput;
  text: string;
}

type Disposition =
  | "compressed"
  | "protected"
  | "recovery_skipped"
  | "not_shorter"
  | "no_match"
  | "failed_open";

export type ObservationOutcome =
  | Disposition
  | "unsupported"
  | "cancelled"
  | "mixed";

export interface Candidate {
  rule: string;
  body_chars: number;
  final_chars: number | null;
  reason: string;
}

interface CoreResponse {
  version: number;
  id: string;
  text?: string;
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
  session: string | null;
  toolCallId: string | null;
  tool: ToolInput;
}

export interface ObservationFacts {
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
  const outcome = dispositions.size > 1 ? "mixed"
    : dispositions.values().next().value ?? "failed_open";

  return {
    parts: results.map((result) => ({
      outcome: result?.disposition ?? "failed_open",
      applied_rule: result?.appliedRule ?? null,
      original_path: result?.originalPath ?? null,
      candidates: result?.candidates ?? [],
      reason: result?.reason ?? (result ? null : "bridge_unavailable"),
    })),
    outcome,
    appliedRule: rules.length === 1 ? rules[0] ?? null : null,
    originalPath: paths.length === 1 ? paths[0] ?? null : null,
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

export function createBridge(
  host: Host,
  executable: string,
  spawnProcess: SpawnProcess,
): {
  process(
    session: string,
    tool: ToolInput,
    text: string,
    signal?: AbortSignalLike,
  ): Promise<ProcessedResult | undefined>;
  recordDelivery(
    original: string | readonly string[],
    outcome: ProcessedResult | "recovery_skipped" | undefined,
    signal?: AbortSignalLike,
  ): void;
  recordObservation(
    context: ObservationContext,
    original: string | null,
    final: string | null,
    facts: ObservationFacts,
  ): void;
  close(): void;
} {
  // Snapshot once when the host loads, including across bridge restarts.
  let recording = false;
  try {
    recording = JSON.parse(readFileSync(join(homedir(), ".outrim", "recording.json"), "utf8")).recording === true;
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

  function process(
    session: string,
    tool: ToolInput,
    text: string,
    signal?: AbortSignalLike,
  ): Promise<ProcessedResult | undefined> {
    if (closed || signal?.aborted) return Promise.resolve(undefined);
    const currentChild = child ?? start();
    if (!currentChild) return Promise.resolve(undefined);

    const id = String(++nextId);
    const request: CoreRequest = { version: 2, id, host, session, tool, text };
    let line: string;
    try {
      line = `${JSON.stringify(request)}\n`;
    } catch {
      return Promise.resolve(undefined);
    }

    return new Promise((resolve) => {
      let settled = false;
      const finish = (response: CoreResponse | undefined) => {
        if (settled) return;
        settled = true;
        signal?.removeEventListener("abort", abort);
        if (
          response &&
          typeof response.text === "string" &&
          typeof response.disposition === "string" &&
          (response.applied_rule === null || typeof response.applied_rule === "string")
        ) {
          resolve({
            text: response.text,
            disposition: response.disposition as Disposition,
            appliedRule: response.applied_rule,
            originalPath: typeof response.original_path === "string" ? response.original_path : null,
            candidates: Array.isArray(response.candidates) ? response.candidates : [],
            reason: typeof response.reason === "string" ? response.reason : null,
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
          if (error) fail(currentChild);
        });
      } catch {
        fail(currentChild);
      }
    });
  }

  // Called only when the host adopts the result. An absent outcome records fail-open;
  // recovery may cover several text parts, but remains one unchanged delivery.
  function recordDelivery(
    original: string | readonly string[],
    outcome: ProcessedResult | "recovery_skipped" | undefined,
    signal?: AbortSignalLike,
  ): void {
    if (closed || signal?.aborted) return;
    const currentChild = child ?? start();
    if (!currentChild) return;

    const texts = typeof original === "string" ? [original] : original;
    const beforeChars = texts.reduce((total, text) => total + Array.from(text).length, 0);
    const processed = typeof outcome === "object" ? outcome : undefined;
    const id = String(++nextId);
    const request = {
      type: "record_delivery",
      version: 2,
      id,
      disposition:
        processed?.disposition ?? (outcome === "recovery_skipped" ? outcome : "failed_open"),
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
      tool: context.tool,
      original_text: original,
      final_text: final,
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

  return { process, recordDelivery, recordObservation, close };
}
