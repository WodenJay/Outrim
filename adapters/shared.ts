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
  input: Record<string, unknown>;
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

export function sourceFor(
  host: Host,
  toolName: string,
  input: Record<string, unknown>,
): Source {
  if (
    ["find", "grep", "ls"].includes(toolName) ||
    (host === "opencode" && ["glob", "list"].includes(toolName))
  ) return "search";
  if (
    toolName !== "bash" &&
    toolName !== "powershell" &&
    !(host === "opencode" && toolName === "shell")
  ) return "unknown";
  const command = input.command;
  if (typeof command !== "string") return "unknown";
  if (matchesAny(TEST_COMMANDS, command)) return "test_log";
  if (matchesAny(BUILD_COMMANDS, command)) return "build_log";
  if (matchesAny(RUNTIME_COMMANDS, command)) return "runtime_log";
  return "unknown";
}

interface CoreRequest {
  version: 1;
  id: string;
  host: Host;
  session: string;
  tool: ToolInput;
  text: string;
}

interface CoreResponse {
  version: number;
  id: string;
  text?: string;
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
  process(session: string, tool: ToolInput, text: string, signal?: AbortSignalLike): Promise<string | undefined>;
  close(): void;
} {
  let child: BridgeChild | undefined;
  let nextId = 0;
  let buffer = "";
  let closed = false;
  const pending = new Map<string, { finish(text: string | undefined): void }>();

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
      if (
        response.version !== 1 ||
        typeof response.id !== "string" ||
        !pending.has(response.id)
      ) {
        fail(failedChild);
        return;
      }
      const request = pending.get(response.id)!;
      pending.delete(response.id);
      request.finish(typeof response.text === "string" ? response.text : undefined);
    }
  }

  function start(): BridgeChild | undefined {
    let started: BridgeChild;
    try {
      started = spawnProcess(executable, ["bridge"], {
        stdio: ["pipe", "pipe", "ignore"],
        windowsHide: true,
      });
    } catch {
      return undefined;
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
  ): Promise<string | undefined> {
    if (closed || signal?.aborted) return Promise.resolve(undefined);
    const currentChild = child ?? start();
    if (!currentChild) return Promise.resolve(undefined);

    const id = String(++nextId);
    const request: CoreRequest = { version: 1, id, host, session, tool, text };
    let line: string;
    try {
      line = `${JSON.stringify(request)}\n`;
    } catch {
      return Promise.resolve(undefined);
    }

    return new Promise((resolve) => {
      let settled = false;
      const finish = (result: string | undefined) => {
        if (settled) return;
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
          if (error) fail(currentChild);
        });
      } catch {
        fail(currentChild);
      }
    });
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
      } finally {
        closingChild.kill();
      }
    }
  }

  return { process, close };
}
