import { describe, expect, it } from "bun:test";
import { EventEmitter } from "node:events";
import { registerPiExtension } from "../assets/outrim-pi.js";
import { isolateHome } from "./isolated-home.mjs";

isolateHome();

class FakeChild extends EventEmitter {
  writes = [];
  killed = false;
  ended = false;
  stdin = new EventEmitter();
  stdout = new EventEmitter();
  constructor() {
    super();
    this.stdin.write = (line, callback) => {
      const request = JSON.parse(line);
      this.writes.push(request);
      callback?.();
      if (request.type?.startsWith("record_")) this.respond(request, {});
      return true;
    };
    this.stdin.end = () => {
      this.ended = true;
    };
    this.stdout.setEncoding = () => {};
  }
  respond(request, fields) {
    this.stdout.emit("data", `${JSON.stringify({ version: 2, id: request.id, ...fields })}\n`);
  }
  kill() {
    this.killed = true;
    return true;
  }
}
function harness(options = {}) {
  const handlers = new Map();
  const children = [];
  registerPiExtension(
    { on: (name, handler) => handlers.set(name, handler) },
    {
      executable: "outrim-test",
      dialect: "bash",
      spawnProcess: () => {
        const child = new FakeChild();
        children.push(child);
        return child;
      },
      ...options,
    },
  );
  const context = { sessionManager: { getSessionId: () => "session-1" } };
  return {
    children,
    handlers,
    call: (event, signal) => handlers.get("tool_call")(event, { ...context, signal }),
    result: (event, signal) => handlers.get("tool_result")(event, { ...context, signal }),
  };
}
function event(overrides = {}) {
  return {
    toolName: "bash",
    toolCallId: "call-1",
    input: { command: "cargo test", timeout: 12 },
    content: [{ type: "text", text: "original 🦀" }],
    details: { keep: true },
    structuredContent: { exit_code: 7 },
    isError: true,
    usage: { output: 9 },
    ...overrides,
  };
}
const processed = (text) => ({ text, disposition: "compressed", applied_rule: "log_tail" });

describe("Pi execution adapter", () => {
  it("mutates the original argument object before native execution", async () => {
    const h = harness();
    const e = event();
    const input = e.input;
    const pending = h.call(e);
    const request = h.children[0].writes[0];
    expect(request).toMatchObject({
      type: "rewrite",
      host: "pi",
      session: "session-1",
      tool_call_id: "call-1",
      unit_id: "call-1:0",
      dialect: "bash",
      recording: false,
      command: "cargo test",
    });
    h.children[0].respond(request, { command: "wrapped command" });
    expect(await pending).toBeUndefined();
    expect(e.input).toBe(input);
    expect(e.input).toEqual({ command: "wrapped command", timeout: 12 });
  });
  it.each(["bash", "powershell", "shell"])(
    "never processes or accounts %s results",
    async (toolName) => {
      const h = harness();
      const e = event({ toolName });
      const before = structuredClone(e);
      expect(await h.result(e)).toBeUndefined();
      expect(e).toEqual(before);
      expect(h.children).toHaveLength(0);
    },
  );
  it.each([null, "error", "exit", "cancel", "shutdown"])(
    "keeps the command unchanged on %s",
    async (failure) => {
      const h = harness();
      const e = event();
      const controller = new AbortController();
      const pending = h.call(e, controller.signal);
      const child = h.children[0];
      if (failure === "error") child.emit("error", new Error("unavailable"));
      else if (failure === "exit") child.emit("exit", 1, null);
      else if (failure === "cancel") {
        controller.abort();
        child.respond(child.writes[0], { command: "late rewrite" });
      } else if (failure === "shutdown") h.handlers.get("session_shutdown")();
      else child.respond(child.writes[0], { command: failure });
      await pending;
      expect(e.input.command).toBe("cargo test");
    },
  );
  it("fails open when the rewrite bridge stops responding", async () => {
    const h = harness();
    const e = event();
    await h.call(e);
    expect(e.input.command).toBe("cargo test");
    expect(h.children[0].killed).toBe(true);
  }, 10_000);
  it("does not overwrite a command changed while awaiting the bridge", async () => {
    const h = harness();
    const e = event();
    const pending = h.call(e);
    e.input.command = "another extension changed this";
    h.children[0].respond(h.children[0].writes[0], { command: "stale rewrite" });
    await pending;
    expect(e.input.command).toBe("another extension changed this");
  });
  it("keeps custom or unknown-dialect shell tools native", async () => {
    const h = harness({ dialect: "unknown" });
    const e = event();
    await h.call(e);
    expect(e.input.command).toBe("cargo test");
    expect(h.children).toHaveLength(0);
  });
  it("leaves nested calls and non-shell calls alone", async () => {
    const h = harness();
    await h.call(event({ parentToolCallId: "parent" }));
    await h.call(event({ toolName: "read" }));
    expect(h.children).toHaveLength(0);
  });
});

describe("Pi native result adapter", () => {
  it("preserves metadata and correlates concurrent native tool results", async () => {
    const h = harness();
    const first = h.result(event({ toolName: "grep", input: { pattern: "hit" } }));
    const second = h.result(event({ toolName: "read", toolCallId: "call-2" }));
    const child = h.children[0];
    expect(child.writes[0].tool.source).toBe("search");
    expect(child.writes[1].tool.source).toBe("unknown");
    child.respond(child.writes[1], processed("original 🦀"));
    child.respond(child.writes[0], processed("short 🦀"));
    expect(await second).toBeUndefined();
    expect(await first).toEqual({
      content: [{ type: "text", text: "short 🦀" }],
      details: { keep: true },
      structuredContent: { exit_code: 7 },
      isError: true,
      usage: { output: 9 },
    });
    const deliveries = child.writes.filter((r) => r.type === "record_delivery");
    expect(deliveries).toHaveLength(2);
    expect(deliveries[1]).toMatchObject({
      before_chars: 10,
      after_chars: 7,
      disposition: "compressed",
    });
  });
  it("protects original references and unsupported content", async () => {
    const h = harness();
    expect(
      await h.result(
        event({ toolName: "read", input: { path: "/home/me/.outrim/outputs/pi/a.txt" } }),
      ),
    ).toBeUndefined();
    expect(h.children[0].writes[0]).toMatchObject({
      type: "record_delivery",
      disposition: "recovery_skipped",
    });
    expect(
      await h.result(event({ toolName: "read", content: [{ type: "image" }] })),
    ).toBeUndefined();
    expect(await h.result(event({ toolName: "read", parentToolCallId: "parent" }))).toBeUndefined();
    expect(h.children[0].writes).toHaveLength(1);
  });
  it("cancels without delivering, then reuses the same bridge", async () => {
    const h = harness();
    const controller = new AbortController();
    const first = h.result(event({ toolName: "read" }), controller.signal);
    const child = h.children[0];
    controller.abort();
    expect(await first).toBeUndefined();
    const second = h.result(event({ toolName: "read" }));
    child.respond(child.writes[0], processed("late"));
    child.respond(child.writes[1], processed("short"));
    expect((await second).content[0].text).toBe("short");
    expect(child.writes.filter((r) => r.type === "record_delivery")).toHaveLength(1);
    expect(child.killed).toBe(false);
    h.handlers.get("session_shutdown")();
    expect(child.ended).toBe(true);
  });
  it("fails open, records delivery, and restarts the bridge", async () => {
    const h = harness();
    const pending = h.result(event({ toolName: "read" }));
    h.children[0].emit("exit", 1, null);
    expect(await pending).toBeUndefined();
    expect(h.children[1].writes[0]).toMatchObject({
      type: "record_delivery",
      disposition: "failed_open",
    });
  });
});
