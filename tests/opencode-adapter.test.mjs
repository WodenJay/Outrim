import { describe, expect, it } from "bun:test";
import { execFileSync } from "node:child_process";
import { EventEmitter } from "node:events";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { registerOpenCodePlugin } from "../adapters/opencode.ts";
import opencodePlugin from "../assets/outrim-opencode.js";
import { isolateHome } from "./isolated-home.mjs";

isolateHome();

class FakeChild extends EventEmitter {
  writes = [];
  ended = false;
  stdin = new EventEmitter();
  stdout = new EventEmitter();

  constructor() {
    super();
    this.stdin.write = (line, callback) => {
      const request = JSON.parse(line);
      this.writes.push(request);
      callback?.();
      if (request.type?.startsWith("record_")) this.respond(request);
      return true;
    };
    this.stdin.end = () => {
      this.ended = true;
    };
    this.stdout.setEncoding = () => {};
  }

  kill() {
    return true;
  }
  requests() {
    return this.writes.filter((request) => !request.type?.startsWith("record_"));
  }
  deliveries() {
    return this.writes.filter((request) => request.type === "record_delivery");
  }
  respond(request, fields = {}) {
    this.stdout.emit("data", `${JSON.stringify({ version: 2, id: request.id, ...fields })}\n`);
  }
  compress(request, text) {
    this.respond(request, { text, disposition: "compressed", applied_rule: "log_tail" });
  }
}

async function harness(options = {}) {
  const handlers = new Map();
  const child = new FakeChild();
  let eventWaiter;
  const dispose = await registerOpenCodePlugin(
    {
      ...Object.fromEntries(
        ["tool", "session", "shell"].map((domain) => [
          domain,
          {
            hook(name, handler) {
              handlers.set(`${domain}.${name}`, handler);
              return Promise.resolve({ dispose: () => handlers.delete(`${domain}.${name}`) });
            },
          },
        ]),
      ),
      event: {
        subscribe({ signal }) {
          signal.addEventListener("abort", () => eventWaiter?.({ done: true }), { once: true });
          return {
            [Symbol.asyncIterator]() {
              return this;
            },
            next() {
              if (signal.aborted) return Promise.resolve({ done: true });
              return new Promise((resolve) => {
                eventWaiter = resolve;
              });
            },
          };
        },
      },
    },
    { spawnProcess: () => child, ...options },
  );
  const call = (name, event) => handlers.get(name)(event);
  const context = (shell = "bash", sessionID = "session", agent = "build") =>
    call("session.context", {
      sessionID,
      agent,
      tools: {
        shell: {
          description: `Execute a shell command and return its output. Commands run on Linux using ${shell}. Quote file paths.`,
        },
      },
    });
  return {
    child,
    handlers,
    dispose,
    call,
    context,
    interrupt(sessionID) {
      eventWaiter({
        value: { type: "session.execution.interrupted", properties: { sessionID } },
        done: false,
      });
    },
  };
}

function before(input = { command: "cargo test" }, overrides = {}) {
  return { tool: "shell", sessionID: "session", agent: "build", id: "call", input, ...overrides };
}
function after(overrides = {}) {
  return {
    ...before(),
    status: "completed",
    result: { content: [{ type: "text", text: "original output" }] },
    ...overrides,
  };
}

// Bridge framing and parsing safety are shared; these tests exercise host adoption.
describe("OpenCode adapter", () => {
  it("keeps the embedded plugin asset in sync with its sources", () => {
    const root = dirname(dirname(fileURLToPath(import.meta.url)));
    const temp = mkdtempSync(join(tmpdir(), "outrim-opencode-"));
    try {
      const asset = join(temp, "outrim-opencode.js");
      execFileSync(
        process.execPath,
        ["build", "adapters/opencode-plugin.ts", "--target=bun", `--outfile=${asset}`],
        { cwd: root, stdio: "ignore" },
      );
      expect(readFileSync(asset, "utf8").replace(/\r\n/g, "\n")).toBe(
        readFileSync(join(root, "assets/outrim-opencode.js"), "utf8").replace(/\r\n/g, "\n"),
      );
      expect(opencodePlugin.id).toBe("outrim");
      expect(typeof opencodePlugin.setup).toBe("function");
    } finally {
      rmSync(temp, { force: true, recursive: true });
    }
  });

  it.each([
    ["bash", "/bin/bash", "bash"],
    ["pwsh", "C:\\PowerShell\\pwsh.exe", "powershell"],
    ["powershell", "powershell.exe", "powershell"],
  ])("rewrites %s input before native execution", async (name, shell, dialect) => {
    const h = await harness();
    h.context(name);
    const event = before({
      command: "cargo test",
      workdir: "workspace",
      timeout: 7,
      background: true,
    });
    const input = event.input;
    const pending = h.call("tool.execute.before", event);
    const [request] = h.child.requests();
    expect(request).toMatchObject({
      type: "rewrite",
      host: "opencode",
      session: "session",
      tool_call_id: "call",
      unit_id: "call:0",
      dialect,
      command: "cargo test",
      recording: false,
    });
    h.child.respond(request, { command: "wrapped command" });
    await pending;
    expect(event.input).toBe(input);
    expect(input).toEqual({
      command: "wrapped command",
      workdir: "workspace",
      timeout: 7,
      background: true,
    });
    const invocation = { shell, command: input.command };
    h.call("shell.create.before", invocation);
    expect(invocation.command).toBe("wrapped command");
    await h.dispose();
  });

  it.each(["cmd.exe", "/bin/zsh", "pwsh.exe"])(
    "restores the original if the launch shell changed to %s",
    async (shell) => {
      const h = await harness();
      h.context();
      const event = before();
      const pending = h.call("tool.execute.before", event);
      h.child.respond(h.child.requests()[0], { command: "wrapped command" });
      await pending;
      const invocation = { shell, command: event.input.command };
      h.call("shell.create.before", invocation);
      expect(invocation.command).toBe("cargo test");
      expect(h.child.deliveries()).toEqual([]);
      await h.dispose();
    },
  );

  it.each(["unknown", "cmd", "zsh"])(
    "does not guess the %s dialect or unknown session/agent",
    async (shell) => {
      const h = await harness();
      h.context(shell);
      for (const overrides of [
        {},
        { sessionID: "other" },
        { agent: "other" },
        { tool: "bash" },
        { input: {} },
      ]) {
        const event = before(undefined, overrides);
        const original = structuredClone(event.input);
        await h.call("tool.execute.before", event);
        expect(event.input).toEqual(original);
      }
      expect(h.child.writes).toEqual([]);
      await h.dispose();
    },
  );

  it("forgets stale descriptions and preserves Code Mode machine consumers", async () => {
    const h = await harness();
    h.context();
    await h.call("tool.execute.before", before({}, { tool: "execute" }));
    await h.call("tool.execute.before", before());
    expect(h.child.writes).toEqual([]);
    await h.call("tool.execute.after", after({ tool: "execute", status: "error" }));
    h.call("session.generate", { sessionID: "session", agent: "build", tools: {} });
    await h.call("tool.execute.before", before());
    expect(h.child.requests()).toEqual([]);
    await h.dispose();
  });

  it.each([{}, { command: null }, { command: "cargo test" }])(
    "leaves input untouched when the bridge declines: %j",
    async (response) => {
      const h = await harness();
      h.context();
      const event = before();
      const pending = h.call("tool.execute.before", event);
      h.child.respond(h.child.requests()[0], response);
      await pending;
      expect(event.input.command).toBe("cargo test");
      expect(h.child.deliveries()).toEqual([]);
      await h.dispose();
    },
  );

  it("passes through startup failure and bridge exit", async () => {
    const h = await harness({
      spawnProcess() {
        throw new Error("missing executable");
      },
    });
    h.context();
    const event = before();
    await h.call("tool.execute.before", event);
    expect(event.input.command).toBe("cargo test");
    await h.dispose();
    const running = await harness();
    running.context();
    const pending = running.call("tool.execute.before", event);
    running.child.emit("exit", 1, null);
    await pending;
    expect(event.input.command).toBe("cargo test");
    expect(running.child.deliveries()).toEqual([]);
    await running.dispose();
  });

  it.each(["interrupt", "dispose"])("does not adopt a pending rewrite after %s", async (action) => {
    const h = await harness();
    h.context();
    const event = before();
    const pending = h.call("tool.execute.before", event);
    if (action === "interrupt") h.interrupt("session");
    else await h.dispose();
    await pending;
    expect(event.input.command).toBe("cargo test");
    await h.dispose();
  });

  it("does not overwrite input changed by another hook while awaiting the bridge", async () => {
    const h = await harness();
    h.context();
    const event = before();
    const pending = h.call("tool.execute.before", event);
    event.input.command = "changed command";
    h.child.respond(h.child.requests()[0], { command: "wrapped command" });
    await pending;
    expect(event.input.command).toBe("changed command");
    await h.dispose();
  });

  it.each(["shell", "bash", "powershell"])(
    "never compresses or accounts for %s results",
    async (tool) => {
      const h = await harness();
      for (const status of ["completed", "error", "cancelled"]) {
        const event = after({ tool, status });
        const result = event.result;
        await h.call("tool.execute.after", event);
        expect(event.result).toBe(result);
      }
      expect(h.child.writes).toEqual([]);
      await h.dispose();
    },
  );

  it("correlates concurrent sessions without changing non-command arguments", async () => {
    const h = await harness();
    h.context("bash", "a");
    h.context("pwsh", "b");
    const a = before(undefined, { sessionID: "a" });
    const b = before(undefined, { sessionID: "b" });
    const pending = [h.call("tool.execute.before", a), h.call("tool.execute.before", b)];
    const [ra, rb] = h.child.requests();
    h.child.respond(rb, { command: "wrapped b" });
    h.child.respond(ra, { command: "wrapped a" });
    await Promise.all(pending);
    expect(a.input.command).toBe("wrapped a");
    expect(b.input.command).toBe("wrapped b");
    expect(ra.dialect).toBe("bash");
    expect(rb.dialect).toBe("powershell");
    await h.dispose();
  });

  it("preserves structured data and per-part delivery for non-shell tools", async () => {
    const h = await harness();
    const file = { type: "file", uri: "file:///tmp/result" };
    const output = { rows: ["caller data"] };
    const metadata = { exit: 1, custom: true };
    const event = after({
      tool: "read",
      input: { filePath: "file.log" },
      result: {
        content: [
          { type: "text", text: "🦀 original" },
          file,
          { type: "text", text: "🙂 unchanged" },
        ],
        output,
        metadata,
      },
    });
    const pending = h.call("tool.execute.after", event);
    const [request, unchanged] = h.child.requests();
    expect(request.tool).toMatchObject({ name: "read", source: "unknown", exit_code: 1 });
    h.child.compress(request, "🦀 short");
    h.child.respond(unchanged, {
      text: "🙂 unchanged",
      disposition: "protected",
      applied_rule: null,
    });
    await pending;
    expect(event.result.content).toEqual([
      { type: "text", text: "🦀 short" },
      file,
      { type: "text", text: "🙂 unchanged" },
    ]);
    expect(event.result.content[1]).toBe(file);
    expect(event.result.output).toBe(output);
    expect(event.result.metadata).toBe(metadata);
    expect(h.child.deliveries()).toMatchObject([
      { disposition: "compressed", before_chars: 10, after_chars: 7 },
      { disposition: "protected", before_chars: 11, after_chars: 11 },
    ]);
    await h.dispose();
  });

  it.each(["grep", "find", "ls", "glob", "list"])(
    "preserves %s search classification",
    async (tool) => {
      const h = await harness();
      const event = after({ tool, input: { pattern: "hit" } });
      const pending = h.call("tool.execute.after", event);
      const [request] = h.child.requests();
      expect(request.tool.source).toBe("search");
      h.child.compress(request, "short");
      await pending;
      expect(event.result.content[0].text).toBe("short");
      await h.dispose();
    },
  );

  it("protects recovered originals but not paths mentioned in output", async () => {
    const h = await harness();
    const event = after({
      tool: "read",
      input: { nested: { filePath: "/home/me/.outrim/outputs/opencode/saved.txt" } },
    });
    const result = event.result;
    await h.call("tool.execute.after", event);
    expect(event.result).toBe(result);
    expect(h.child.requests()).toEqual([]);
    expect(h.child.deliveries()).toMatchObject([{ disposition: "recovery_skipped" }]);
    expect(h.child.writes.find((r) => r.type === "record_recovery")).toMatchObject({
      attribution: "unattributed",
    });
    event.input = { filePath: "normal.log" };
    event.result = { output: "mentioned ~/.outrim/outputs/file.txt" };
    const pending = h.call("tool.execute.after", event);
    h.child.compress(h.child.requests()[0], "short");
    await pending;
    expect(event.result.content).toBe("short");
    expect(event.result.output).toBe("mentioned ~/.outrim/outputs/file.txt");
    await h.dispose();
  });

  it("preserves Code Mode nesting across colliding session call IDs", async () => {
    const h = await harness();
    for (const sessionID of ["a", "b", "a"])
      await h.call("tool.execute.before", before({}, { tool: "execute", sessionID }));
    for (const event of [
      after({ tool: "read", sessionID: "a" }),
      after({ tool: "execute", sessionID: "a" }),
    ]) {
      const result = event.result;
      await h.call("tool.execute.after", event);
      expect(event.result).toBe(result);
    }
    expect(h.child.requests()).toEqual([]);
    h.interrupt("a");
    await Promise.resolve();
    const nested = after({ tool: "read", sessionID: "b" });
    await h.call("tool.execute.after", nested);
    expect(h.child.requests()).toEqual([]);
    const outer = after({ tool: "execute", sessionID: "b" });
    const pending = h.call("tool.execute.after", outer);
    h.child.compress(h.child.requests()[0], "short summary");
    await pending;
    expect(outer.result.content[0].text).toBe("short summary");
    await h.dispose();
  });

  it("releases pending non-shell results on unload without adopting them", async () => {
    const h = await harness();
    const event = after({ tool: "read" });
    const result = event.result;
    const pending = h.call("tool.execute.after", event);
    await h.dispose();
    await pending;
    expect(event.result).toBe(result);
    expect(h.child.ended).toBe(true);
    expect(h.handlers.size).toBe(0);
  });
});
