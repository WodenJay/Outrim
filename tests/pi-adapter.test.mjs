import { EventEmitter } from "node:events";
import { describe, expect, it } from "bun:test";
import { registerPiExtension } from "../assets/outrim-pi.js";

class FakeChild extends EventEmitter {
  writes = [];
  killed = false;
  stdin = new EventEmitter();
  stdout = new EventEmitter();

  constructor() {
    super();
    this.stdin.write = (line, callback) => {
      this.writes.push(line);
      callback?.();
      return true;
    };
    this.stdin.end = () => {};
    this.stdout.setEncoding = () => {};
  }

  requests() {
    return this.writes.map((line) => JSON.parse(line));
  }

  respond(request, text) {
    this.stdout.emit(
      "data",
      `${JSON.stringify({ version: 1, id: request.id, text, disposition: "compressed" })}\n`,
    );
  }

  exit() {
    this.emit("exit", 1, null);
  }

  kill() {
    this.killed = true;
    this.emit("exit", null, "SIGTERM");
    return true;
  }
}

function harness() {
  const handlers = new Map();
  const children = [];
  const pi = {
    on(event, handler) {
      handlers.set(event, handler);
    },
  };
  registerPiExtension(pi, {
    executable: "outrim-test",
    spawnProcess: () => {
      const child = new FakeChild();
      children.push(child);
      return child;
    },
  });
  const handle = (toolEvent, signal) =>
    handlers.get("tool_result")(toolEvent, {
      signal,
      sessionManager: { getSessionId: () => "session-1" },
    });
  return { handlers, children, handle };
}

function toolEvent(overrides = {}) {
  return {
    toolName: "bash",
    toolCallId: "call-1",
    input: { command: "cargo test" },
    content: [{ type: "text", text: "original output" }],
    details: {},
    structuredContent: { exit_code: 1 },
    isError: true,
    usage: { input: 4, output: 5 },
    ...overrides,
  };
}

describe("Pi result adapter", () => {
  it("reuses the bridge and correlates concurrent results by request ID", async () => {
    const { children, handle } = harness();
    const first = handle(toolEvent());
    const second = handle(
      toolEvent({
        toolCallId: "call-2",
        input: { command: "cargo build" },
        content: [{ type: "text", text: "second original" }],
      }),
    );
    const third = handle(
      toolEvent({
        toolCallId: "call-3",
        input: { command: 'echo "vitest"' },
        content: [{ type: "text", text: "cargo test output stays unknown" }],
      }),
    );

    expect(children).toHaveLength(1);
    const requests = children[0].requests();
    expect(requests).toHaveLength(3);
    expect(requests[0]).toMatchObject({
      version: 1,
      host: "pi",
      session: "session-1",
      tool: {
        name: "bash",
        input: { command: "cargo test" },
        source: "test_log",
        exit_code: 1,
      },
      text: "original output",
    });
    expect(requests[1].tool.source).toBe("build_log");
    expect(requests[2].tool.source).toBe("unknown");

    children[0].respond(requests[1], "compressed second");
    children[0].respond(requests[2], "cargo test output stays unknown");
    children[0].respond(requests[0], "compressed first");
    const [firstResult, secondResult, thirdResult] = await Promise.all([first, second, third]);

    expect(firstResult.content[0].text).toBe("compressed first");
    expect(secondResult.content[0].text).toBe("compressed second");
    expect(thirdResult).toBeUndefined();
    expect(firstResult.details).toEqual({});
    expect(firstResult.structuredContent).toEqual({ exit_code: 1 });
    expect(firstResult.isError).toBe(true);
    expect(firstResult.usage).toEqual({ input: 4, output: 5 });
  });

  it("skips paths found in tool input without inspecting output text", async () => {
    const { children, handle } = harness();
    const recovery = await handle(
      toolEvent({
        input: { nested: ["C:\\Users\\me\\.outrim\\outputs\\pi\\saved.txt"] },
      }),
    );
    expect(recovery).toBeUndefined();
    for (const path of [
      "/home/me/.outrim/outputs/pi/saved.txt",
      'Get-Content "$HOME/.outrim\\outputs\\pi\\saved.txt"',
      "~/.outrim/outputs/pi/saved.txt",
      "cat .outrim/outputs/pi/saved.txt",
      "ls .outrim/outputs",
      "Get-Content .outrim\\outputs\\pi\\saved.txt",
    ]) {
      expect(await handle(toolEvent({ input: { nested: { path } } }))).toBeUndefined();
    }
    expect(children).toHaveLength(0);

    const outputMentionsDirectory = handle(
      toolEvent({
        input: { command: "cargo test" },
        content: [{ type: "text", text: "output mentions ~/.outrim/outputs/saved.txt" }],
      }),
    );
    const request = children[0].requests()[0];
    children[0].respond(request, "compressed output");
    expect((await outputMentionsDirectory).content[0].text).toBe("compressed output");
    expect(children).toHaveLength(1);
  });

  it("preserves structured data and leaves image and nested results untouched", async () => {
    const { children, handle } = harness();

    expect(
      await handle(
        toolEvent({
          content: [
            { type: "text", text: "text" },
            { type: "image", data: "opaque-image-data" },
          ],
        }),
      ),
    ).toBeUndefined();
    expect(await handle(toolEvent({ parentToolCallId: "parent-1" }))).toBeUndefined();
    expect(children).toHaveLength(0);

    const structuredContent = { value: [1, "two"] };
    const pending = handle(toolEvent({ structuredContent }));
    const [request] = children[0].requests();
    children[0].respond(request, "compressed output");
    const result = await pending;
    expect(result.content[0].text).toBe("compressed output");
    expect(result.structuredContent).toEqual(structuredContent);
    expect(children).toHaveLength(1);
  });

  it("keeps a shared child alive after one request is cancelled", async () => {
    const { children, handle } = harness();
    const controller = new AbortController();
    const cancelled = handle(toolEvent(), controller.signal);
    const child = children[0];
    const [cancelledRequest] = child.requests();

    controller.abort();
    expect(await cancelled).toBeUndefined();
    expect(child.killed).toBe(false);

    const next = handle(toolEvent({ toolCallId: "call-2" }));
    const nextRequest = child.requests()[1];
    child.respond(cancelledRequest, "late cancelled result");
    child.respond(nextRequest, "next result");

    expect((await next).content[0].text).toBe("next result");
    expect(children).toHaveLength(1);
  });

  it("fails open on child exit and lazily restarts on the next result", async () => {
    const { children, handle } = harness();
    const pending = handle(toolEvent());
    children[0].exit();
    expect(await pending).toBeUndefined();

    const restarted = handle(toolEvent({ toolCallId: "call-after-exit" }));
    expect(children).toHaveLength(2);
    const [request] = children[1].requests();
    children[1].respond(request, "restarted result");
    expect((await restarted).content[0].text).toBe("restarted result");
  });

  it("kills its child and resolves pending work on session shutdown", async () => {
    const { handlers, children, handle } = harness();
    const pending = handle(toolEvent());
    const child = children[0];

    handlers.get("session_shutdown")();
    expect(await pending).toBeUndefined();
    expect(child.killed).toBe(true);
    expect(children).toHaveLength(1);
  });
});
