import { describe, expect, it } from "bun:test";
import { EventEmitter } from "node:events";
import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { registerPiExtension } from "../assets/outrim-pi.js";
import { isolateHome } from "./isolated-home.mjs";

isolateHome();

class FakeChild extends EventEmitter {
  writes = [];
  selection = "builtin";
  deferPolicy = false;
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
      if (
        request.type?.startsWith("record_") ||
        ["read_reset", "read_delivered"].includes(request.type)
      )
        this.respond(request, {});
      if (request.type === "resolve_policy" && !this.deferPolicy)
        this.respond(request, { selection: this.selection });
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
  const context = { cwd: "D:/host-session", sessionManager: { getSessionId: () => "session-1" } };
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
const processed = (text) => ({ text, disposition: "compressed", applied_rule: "log_compaction" });

describe("Pi execution adapter", () => {
  it.each(["session_start", "session_compact", "session_tree"])(
    "does not revive an invocation awaiting policy after %s",
    async (lifecycle) => {
      const child = new FakeChild();
      child.deferPolicy = true;
      const h = harness({ spawnProcess: () => child });
      const e = event({ toolName: "read", input: { path: "data.json" } });
      const before = h.call(e);
      const policy = child.writes.find((message) => message.type === "resolve_policy");
      h.handlers.get(lifecycle)();
      child.respond(policy, { selection: "builtin" });
      await before;
      const pending = h.result(e);
      const batch = child.writes.find((message) => message.type === "process_parts");
      expect(batch.selection).toBe("off");
      child.respond(batch, {
        parts: [{ text: "original 🦀", disposition: "protected", applied_rule: null }],
      });
      expect(await pending).toBeUndefined();
      h.handlers.get("session_shutdown")();
    },
  );
  it("keeps a failed policy snapshot off even if the bridge recovers", async () => {
    for (const failure of ["spawn", "missing-selection"]) {
      let first = true;
      const child = new FakeChild();
      child.selection = undefined;
      const h = harness({
        spawnProcess: () => {
          if (first && failure === "spawn") {
            first = false;
            throw new Error("bridge unavailable");
          }
          return child;
        },
      });
      const e = event({ toolName: "read", input: { path: "data.json" } });
      await h.call(e);
      child.selection = "builtin";
      const pending = h.result(e);
      const batch = child.writes.find((message) => message.type === "process_parts");
      expect(batch.selection).toBe("off");
      child.respond(batch, { parts: [{ text: "original 🦀", disposition: "unchanged" }] });
      await pending;
      h.handlers.get("session_shutdown")();
    }
  });
  it("pins policy before execution and refreshes only for the next call", async () => {
    const h = harness();
    const e = event({ toolName: "read", input: { path: "data.json" } });
    for (const selection of ["builtin", "off"]) {
      await h.call(e);
      const child = h.children[0];
      const snapshot = child.writes.filter((message) => message.type === "resolve_policy").at(-1);
      expect(snapshot.request.context).toEqual({ cwd: "D:/host-session" });
      expect(snapshot.request.tool.input).toEqual(e.input);
      child.selection = "off";
      const pending = h.result(e);
      const batch = child.writes.filter((message) => message.type === "process_parts").at(-1);
      expect(batch.selection).toEqual(selection);
      child.respond(batch, { parts: [processed("short")] });
      await pending;
    }
    h.handlers.get("session_shutdown")();
  });
  it("uses host cwd rather than read targets or input directories", async () => {
    const h = harness();
    const input = { path: "Z:/outside/data.json", directory: "Z:/not-session" };
    const pending = h.result(event({ toolName: "read", input }));
    const request = h.children[0].writes.find((message) => !message.type);
    expect(request.context).toEqual({ cwd: "D:/host-session" });
    expect(request.tool).toMatchObject({ name: "read", input });
    h.children[0].respond(request, processed("short"));
    await pending;
    h.handlers.get("session_shutdown")();
  });
  it("retains only explicit host truncation facts on metadata-only results", async () => {
    const settings = join(homedir(), ".outrim", "recording.json");
    mkdirSync(join(homedir(), ".outrim"), { recursive: true });
    writeFileSync(settings, JSON.stringify({ recording: true }));
    const h = harness();
    rmSync(settings);
    const e = event({
      details: {
        truncation: {
          truncated: true,
          truncatedBy: "bytes",
          totalBytes: 90000,
          outputBytes: 51200,
          maxBytes: 51200,
          content: "private payload",
        },
      },
    });
    const original = structuredClone(e);
    expect(await h.result(e)).toBeUndefined();
    expect(e).toEqual(original);
    const record = h.children[0].writes.find((request) => request.type === "record_observation");
    expect(record.diagnostics).toMatchObject({
      completeness: "host_truncated",
      host_truncation: { totalBytes: 90000, outputBytes: 51200, maxBytes: 51200 },
      observed_chars: 10,
      observed_bytes: 13,
    });
    expect(record.diagnostics.total_bytes).toBeUndefined();
    expect(JSON.stringify(record)).not.toContain("private payload");
    expect(record.original_text).toBeNull();
  });
  it("records wrapping stages without retaining newly observed inputs", async () => {
    const settings = join(homedir(), ".outrim", "recording.json");
    mkdirSync(join(homedir(), ".outrim"), { recursive: true });
    writeFileSync(settings, JSON.stringify({ recording: true }));
    const h = harness();
    rmSync(settings);
    const e = event();
    const pending = h.call(e);
    const child = h.children[0];
    child.respond(
      child.writes.find((r) => r.type === "rewrite"),
      { command: "wrapped" },
    );
    await pending;
    const records = child.writes.filter((r) => r.type === "record_observation");
    expect(records.map((r) => r.diagnostics.wrapping)).toEqual(["planned", "adopted"]);
    for (const record of records) {
      expect(record.tool.input).toBeNull();
      expect(record.original_text).toBeNull();
      expect(record.metadata_only).toBe(true);
      expect(record.diagnostics.adapter_build).toMatch(/^[a-f0-9]{64}$/);
      expect(JSON.stringify(record)).not.toContain("cargo test");
    }
  });
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
      tool: { name: "bash", input: { command: "cargo test", timeout: 12 }, exit_code: null },
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
  it("keeps nested calls native and only snapshots non-shell policy", async () => {
    const h = harness();
    await h.call(event({ parentToolCallId: "parent" }));
    expect(h.children).toHaveLength(0);
    await h.call(event({ toolName: "read" }));
    expect(h.children[0].writes.map((request) => request.type)).toEqual(["resolve_policy"]);
    h.handlers.get("session_shutdown")();
  });
});

describe("Pi native result adapter", () => {
  it("passes complete source lines through recognized host truncation, retaining the notice", async () => {
    const path = join(homedir(), "paginated source.txt");
    writeFileSync(path, "one\ntwo\nthree");
    const h = harness();
    const text = "one\ntwo\n\n[Showing lines 1-2 of 3. Use offset=3 to continue.]";
    const pending = h.result(
      event({
        toolName: "read",
        input: { path },
        isError: false,
        content: [{ type: "text", text }],
        details: { truncation: { truncated: true, truncatedBy: "lines", lastLinePartial: false } },
      }),
    );
    const child = h.children[0];
    const request = child.writes[0];
    expect(request.type).toBe("read_process");
    expect(request.text).toBe(text);
    expect(request.selection.body.lines).toEqual([{ text: "one\n", rendered: "one\n" }]);
    expect(request.selection.body.suffix).toBe(
      "two\n\n[Showing lines 1-2 of 3. Use offset=3 to continue.]",
    );
    child.respond(request, { text, disposition: "protected", applied_rule: null });
    await pending;
    h.handlers.get("session_shutdown")();
  });
  it("supplies native read facts and adopts only delivered bodies across compacts", async () => {
    const path = join(homedir(), "native source.txt");
    writeFileSync(path, "source");
    const h = harness();
    const e = event({
      toolName: "read",
      input: { path, offset: 10, limit: 300 },
      content: [{ type: "text", text: "line\n".repeat(291) }],
      isError: false,
    });
    const pending = h.result(e);
    const child = h.children[0];
    const request = child.writes[0];
    expect(request.type).toBe("read_process");
    expect(request.selection).toMatchObject({ label: "10–300" });
    child.respond(request, { text: request.text, disposition: "protected", applied_rule: null });
    expect(await pending).toBeUndefined();
    expect(child.writes.find((r) => r.type === "read_delivered").request_id).toBe(request.id);
    for (const lifecycle of ["session_compact", "session_tree", "session_start"])
      h.handlers.get(lifecycle)();
    for (const isError of [false, true]) {
      const compact = h.result(event({ toolName: "compress", isError }));
      child.respond(child.writes.at(-1), {
        text: "original 🦀",
        disposition: "no_match",
        applied_rule: null,
      });
      await compact;
    }
    expect(child.writes.filter((r) => r.type === "read_reset")).toHaveLength(5);
    const late = h.result(e);
    const before = child.writes.at(-1);
    h.handlers.get("session_compact")();
    child.respond(before, processed("stale omission"));
    expect(await late).toBeUndefined();
    expect(child.writes.filter((r) => r.type === "read_delivered")).toHaveLength(1);
    h.handlers.get("session_shutdown")();
  });
  it("charges returned Unicode text per successful recovery, including shell results", async () => {
    const directory = join(homedir(), ".outrim", "outputs", "pi");
    mkdirSync(directory, { recursive: true });
    const path = join(directory, "original.txt");
    writeFileSync(path, "original content");
    const h = harness();
    const result = event({
      toolName: "read",
      input: { path, offset: 2, limit: 1 },
      content: [{ type: "text", text: "2: 🦀\n[continued]" }],
      isError: false,
    });
    expect(await h.result(result)).toBeUndefined();
    expect(await h.result(result)).toBeUndefined();
    expect(await h.result({ ...result, toolCallId: "call-2" })).toBeUndefined();
    expect(await h.result({ ...result, toolCallId: "failed", isError: true })).toBeUndefined();
    const shell = event({
      toolCallId: "shell-read",
      input: { command: `head -n 1 '${path}'` },
      structuredContent: { exit_code: 0 },
      isError: false,
    });
    const unchanged = structuredClone(shell);
    expect(await h.result(shell)).toBeUndefined();
    expect(shell).toEqual(unchanged);
    expect(
      await h.result({ ...shell, toolCallId: "failed-shell", structuredContent: { exit_code: 1 } }),
    ).toBeUndefined();
    const recoveries = h.children[0].writes.filter((r) => r.type === "record_recovery");
    expect(recoveries).toHaveLength(4);
    expect(recoveries[0]).toMatchObject({ text: "2: 🦀\n[continued]", attribution: "confirmed" });
    expect(recoveries[0].event_id).toBe(recoveries[1].event_id);
    expect(recoveries[2].event_id).not.toBe(recoveries[0].event_id);
    expect(recoveries[3]).toMatchObject({
      text: "original 🦀",
      attribution: "confirmed",
      original_paths: [path],
    });
    expect(h.children[0].writes.some((r) => !r.type)).toBe(false);
  });
  it("bundles the single-text observation only when recording is enabled", async () => {
    const settings = join(homedir(), ".outrim", "recording.json");
    mkdirSync(join(homedir(), ".outrim"), { recursive: true });
    writeFileSync(settings, JSON.stringify({ recording: true }));
    try {
      const h = harness();
      const pending = h.result(event({ toolName: "grep", input: { pattern: "hit" } }));
      const child = h.children[0];
      child.respond(child.writes[0], processed("short 🦀\n[recovery hint]"));
      await pending;
      const delivery = child.writes.find((r) => r.type === "record_delivery");
      expect(delivery).toMatchObject({
        original_text: "original 🦀",
        final_text: "short 🦀\n[recovery hint]",
        observation: {
          type: "record_observation",
          version: 2,
          id: "",
          original_text: "original 🦀",
          final_text: "short 🦀\n[recovery hint]",
          outcome: "compressed",
        },
      });
      expect(child.writes.filter((r) => r.type === "record_observation")).toHaveLength(0);
    } finally {
      rmSync(settings, { force: true });
    }
  });
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
      original_text: "original 🦀",
      final_text: "short 🦀",
      disposition: "compressed",
    });
    expect(deliveries[1]).not.toHaveProperty("before_chars");
    expect(deliveries[1]).not.toHaveProperty("after_chars");
    expect(deliveries[1]).not.toHaveProperty("observation");
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
