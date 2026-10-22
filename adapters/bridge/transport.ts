export interface AbortSignalLike {
  readonly aborted: boolean;
  addEventListener(type: "abort", listener: () => void, options?: { once?: boolean }): void;
  removeEventListener(type: "abort", listener: () => void): void;
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

interface Envelope {
  version: number;
  id: string;
}

/** Owns the child and NDJSON framing; callers own the meaning of each message. */
export function createTransport<Response extends Envelope>(
  executable: string,
  args: string[],
  spawnProcess: SpawnProcess,
) {
  let child: BridgeChild | undefined;
  let nextId = 0;
  let buffer = "";
  let closed = false;
  const pending = new Map<string, { finish(response: Response | undefined): void }>();

  function fail(failedChild: BridgeChild): void {
    if (child !== failedChild) return;
    child = undefined;
    buffer = "";
    for (const request of pending.values()) request.finish(undefined);
    pending.clear();
    failedChild.kill();
  }

  function receive(currentChild: BridgeChild, chunk: string): void {
    if (child !== currentChild) return;
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
        fail(currentChild);
        return;
      }
      if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
        fail(currentChild);
        return;
      }
      const response = parsed as Response;
      const request = typeof response.id === "string" ? pending.get(response.id) : undefined;
      if (response.version !== 2 || !request) {
        fail(currentChild);
        return;
      }
      pending.delete(response.id);
      request.finish(response);
    }
  }

  function start(): BridgeChild | undefined {
    let started: BridgeChild;
    try {
      started = spawnProcess(executable, args, {
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
  ): Promise<Response | undefined> {
    if (closed || signal?.aborted) return Promise.resolve(undefined);
    const currentChild = child ?? start();
    if (!currentChild) return Promise.resolve(undefined);

    const id = String(++nextId);
    const request = {
      ...payload,
      version: 2,
      id,
      ...(payload.type === "process_parts"
        ? { request: { ...(payload.request as Record<string, unknown>), id } }
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
      const finish = (response: Response | undefined) => {
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
    exchange,
    close,
    get running(): boolean {
      return child !== undefined;
    },
  };
}
