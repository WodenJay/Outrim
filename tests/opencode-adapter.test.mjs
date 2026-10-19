import { execFileSync } from "node:child_process";
import { EventEmitter } from "node:events";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "bun:test";
import opencodePlugin from "../assets/outrim-opencode.js";
import { registerOpenCodePlugin } from "../adapters/opencode.ts";

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
    this.stdout.emit("data", `${JSON.stringify({ version: 1, id: request.id, text })}\n`);
  }

  kill() {
    this.killed = true;
    this.emit("exit", null, "SIGTERM");
    return true;
  }
}

function harness(options, subscribe = false) {
  const handlers = new Map();
  const queuedEvents = [];
  let eventWaiter;
  const context = {
    tool: {
      hook(name, handler) {
        handlers.set(name, handler);
        return Promise.resolve({ dispose: async () => handlers.delete(name) });
      },
    },
  };
  if (subscribe) {
    context.event = {
      subscribe({ signal }) {
        signal.addEventListener("abort", () => {
          if (!eventWaiter) return;
          const resolve = eventWaiter;
          eventWaiter = undefined;
          resolve({ done: true });
        }, { once: true });
        return {
          [Symbol.asyncIterator]() {
            return this;
          },
          next() {
            if (queuedEvents.length > 0) {
              return Promise.resolve({ value: queuedEvents.shift(), done: false });
            }
            if (signal.aborted) return Promise.resolve({ done: true });
            return new Promise((resolve) => {
              eventWaiter = resolve;
            });
          },
          return() {
            return Promise.resolve({ done: true });
          },
        };
      },
    };
  }
  return {
    handlers,
    cleanup: registerOpenCodePlugin(context, options),
    emitEvent(event) {
      if (eventWaiter) {
        const resolve = eventWaiter;
        eventWaiter = undefined;
        resolve({ value: event, done: false });
      } else {
        queuedEvents.push(event);
      }
    },
  };
}

describe("OpenCode result adapter", () => {
  it("keeps the embedded plugin asset in sync with its sources", () => {
    const projectRoot = dirname(dirname(fileURLToPath(import.meta.url)));
    const tempDir = mkdtempSync(join(tmpdir(), "outrim-opencode-"));
    const generatedAsset = join(tempDir, "outrim-opencode.js");
    try {
      execFileSync(
        process.execPath,
        [
          "build",
          "adapters/opencode-plugin.ts",
          "--target=bun",
          `--outfile=${generatedAsset}`,
        ],
        { cwd: projectRoot, stdio: "ignore" },
      );
      const normalizeLineEndings = (text) => text.replace(/\r\n/g, "\n");
      expect(normalizeLineEndings(readFileSync(generatedAsset, "utf8"))).toBe(
        normalizeLineEndings(
          readFileSync(join(projectRoot, "assets/outrim-opencode.js"), "utf8"),
        ),
      );
    } finally {
      rmSync(tempDir, { force: true, recursive: true });
    }
  });

  it("exports the v2 plugin loader shape from the embedded asset", () => {
    expect(opencodePlugin.id).toBe("outrim");
    expect(typeof opencodePlugin.setup).toBe("function");
  });

  it("changes only model-visible text and preserves structured tool data", async () => {
    const child = new FakeChild();
    const { handlers, cleanup } = harness({
      executable: "outrim-test",
      spawnProcess: () => child,
    });
    const dispose = await cleanup;
    const metadata = { exit: 1, custom: { retained: true } };
    const structuredOutput = { rows: ["kept"] };
    const result = {
      content: [{ type: "text", text: "original output" }],
      output: structuredOutput,
      metadata,
    };
    const event = {
      status: "completed",
      tool: "shell",
      sessionID: "session-1",
      id: "call-1",
      input: { command: "cargo test" },
      result,
    };

    const pending = handlers.get("execute.after")(event);
    const [request] = child.requests();
    expect(request).toMatchObject({
      version: 1,
      host: "opencode",
      session: "session-1",
      tool: {
        name: "shell",
        input: { command: "cargo test" },
        source: "test_log",
        exit_code: 1,
      },
      text: "original output",
    });

    child.respond(request, "compressed output");
    await pending;

    expect(event.result?.content).toEqual([{ type: "text", text: "compressed output" }]);
    expect(event.result?.output).toBe(structuredOutput);
    expect(event.result?.metadata).toBe(metadata);
    await dispose();
  });

  it("preserves Code Mode inputs across colliding sessions and nested calls", async () => {
    const child = new FakeChild();
    const { handlers, cleanup } = harness({
      executable: "outrim-test",
      spawnProcess: () => child,
    });
    const dispose = await cleanup;
    const id = "code-mode-call";
    const sessionA = "session-code-mode-a";
    const sessionB = "session-code-mode-b";
    for (const sessionID of [sessionA, sessionB]) {
      await handlers.get("execute.before")({ tool: "execute", sessionID, id });
    }

    const executeEventB = {
      status: "completed",
      tool: "execute",
      sessionID: sessionB,
      id,
      input: { code: "return await tools.read({ filePath: 'src/main.rs' })" },
      result: {
        content: [{ type: "text", text: "program summary B" }],
        output: { output: "program summary B", toolCalls: [] },
      },
    };
    const outputB = executeEventB.result.output;
    const pendingB = handlers.get("execute.after")(executeEventB);
    const [requestB] = child.requests();
    expect(requestB.text).toBe("program summary B");
    child.respond(requestB, "compressed summary B");
    await pendingB;

    const nestedReadResultBeforeExecute = {
      content: [{ type: "text", text: "raw nested read result" }],
      output: { rows: ["raw data for the program"] },
    };
    const nestedReadBeforeExecute = {
      status: "completed",
      tool: "read",
      sessionID: sessionA,
      id,
      input: { filePath: "src/main.rs" },
      result: nestedReadResultBeforeExecute,
    };
    const pendingNestedReadBeforeExecute = handlers.get("execute.after")(
      nestedReadBeforeExecute,
    );
    const afterNestedReadBeforeExecute = child.requests();
    for (const request of afterNestedReadBeforeExecute.slice(1)) {
      child.respond(request, "unexpected compression");
    }
    await pendingNestedReadBeforeExecute;
    expect(afterNestedReadBeforeExecute).toHaveLength(1);
    expect(nestedReadBeforeExecute.result).toBe(nestedReadResultBeforeExecute);

    await handlers.get("execute.before")({ tool: "execute", sessionID: sessionA, id });
    const nestedExecuteResult = {
      content: [{ type: "text", text: "raw nested execute result" }],
      output: { rows: ["raw data for the program"] },
    };
    const nestedExecuteEvent = {
      status: "completed",
      tool: "execute",
      sessionID: sessionA,
      id,
      input: { code: "return await tools.read({ filePath: 'src/main.rs' })" },
      result: nestedExecuteResult,
    };
    const nestedExecutePending = handlers.get("execute.after")(nestedExecuteEvent);
    const afterNestedExecute = child.requests();
    for (const request of afterNestedExecute.slice(1)) {
      child.respond(request, "unexpected compression");
    }
    await nestedExecutePending;
    expect(afterNestedExecute).toHaveLength(1);
    expect(nestedExecuteEvent.result).toBe(nestedExecuteResult);

    const nestedReadResult = {
      content: [{ type: "text", text: "raw nested read result" }],
      output: { rows: ["raw data for the program"] },
    };
    const nestedReadEvent = {
      status: "completed",
      tool: "read",
      sessionID: sessionA,
      id,
      input: { filePath: "src/main.rs" },
      result: nestedReadResult,
    };
    const nestedReadPending = handlers.get("execute.after")(nestedReadEvent);
    const afterNestedRead = child.requests();
    for (const request of afterNestedRead.slice(1)) {
      child.respond(request, "unexpected compression");
    }
    await nestedReadPending;
    expect(afterNestedRead).toHaveLength(1);
    expect(nestedReadEvent.result).toBe(nestedReadResult);

    const executeEventA = {
      status: "completed",
      tool: "execute",
      sessionID: sessionA,
      id,
      input: { code: "return await tools.read({ filePath: 'src/main.rs' })" },
      result: {
        content: [{ type: "text", text: "program summary A" }],
        output: { output: "program summary A", toolCalls: [] },
      },
    };
    const outputA = executeEventA.result.output;
    const pendingA = handlers.get("execute.after")(executeEventA);
    const [, requestA] = child.requests();
    expect(requestA.text).toBe("program summary A");
    child.respond(requestA, "compressed summary A");
    await pendingA;

    expect(executeEventA.result.content).toEqual([
      { type: "text", text: "compressed summary A" },
    ]);
    expect(executeEventA.result.output).toBe(outputA);
    expect(executeEventB.result.content).toEqual([
      { type: "text", text: "compressed summary B" },
    ]);
    expect(executeEventB.result.output).toBe(outputB);
    await dispose();
  });

  it("correlates concurrent tool results by bridge request ID", async () => {
    const child = new FakeChild();
    const { handlers, cleanup } = harness({
      executable: "outrim-test",
      spawnProcess: () => child,
    });
    const dispose = await cleanup;
    const first = {
      status: "completed",
      tool: "shell",
      sessionID: "session-a",
      id: "call-a",
      input: { command: "cargo test" },
      result: { content: [{ type: "text", text: "first output" }] },
    };
    const second = {
      status: "completed",
      tool: "shell",
      sessionID: "session-b",
      id: "call-b",
      input: { command: "cargo test" },
      result: { content: [{ type: "text", text: "second output" }] },
    };

    const firstPending = handlers.get("execute.after")(first);
    const secondPending = handlers.get("execute.after")(second);
    const [firstRequest, secondRequest] = child.requests();
    expect(firstRequest.id).not.toBe(secondRequest.id);
    child.respond(secondRequest, "compressed second");
    child.respond(firstRequest, "compressed first");
    await Promise.all([firstPending, secondPending]);

    expect(first.result?.content).toEqual([{ type: "text", text: "compressed first" }]);
    expect(second.result?.content).toEqual([{ type: "text", text: "compressed second" }]);
    await dispose();
  });

  it("rewrites text parts without changing file or structured output parts", async () => {
    const child = new FakeChild();
    const { handlers, cleanup } = harness({
      executable: "outrim-test",
      spawnProcess: () => child,
    });
    const dispose = await cleanup;
    const file = { type: "file", uri: "file:///tmp/result", mime: "text/plain" };
    const structuredOutput = { code: "used by caller" };
    const event = {
      status: "completed",
      tool: "shell",
      sessionID: "session-2",
      id: "call-2",
      input: { command: "cargo build" },
      result: {
        content: [{ type: "text", text: "log output" }, file],
        output: structuredOutput,
        metadata: { exit: 0 },
      },
    };

    const pending = handlers.get("execute.after")(event);
    const [request] = child.requests();
    child.respond(request, "short output");
    await pending;

    expect(event.result?.content).toEqual([
      { type: "text", text: "short output" },
      file,
    ]);
    expect(event.result?.content[1]).toBe(file);
    expect(event.result?.output).toBe(structuredOutput);
    await dispose();
  });

  it("skips original-output paths in input, not paths mentioned in output", async () => {
    const child = new FakeChild();
    const { handlers, cleanup } = harness({
      executable: "outrim-test",
      spawnProcess: () => child,
    });
    const dispose = await cleanup;
    const recovered = {
      status: "completed",
      tool: "read",
      sessionID: "session-3",
      id: "recover",
      input: { nested: { filePath: "/home/me/.outrim/outputs/opencode/saved.txt" } },
      result: { content: [{ type: "text", text: "recovered original" }] },
    };
    await handlers.get("execute.after")(recovered);
    expect(child.requests()).toHaveLength(0);

    const outputMentionsPath = {
      ...recovered,
      id: "normal",
      input: { command: "cargo test" },
      result: { content: [{ type: "text", text: "large output mentioning ~/.outrim/outputs/file.txt" }] },
    };
    const pending = handlers.get("execute.after")(outputMentionsPath);
    const [request] = child.requests();
    child.respond(request, "compressed output");
    await pending;
    expect(outputMentionsPath.result?.content).toEqual([
      { type: "text", text: "compressed output" },
    ]);
    await dispose();
  });

  it("adds compressed content without changing a string or structured output value", async () => {
    const child = new FakeChild();
    const { handlers, cleanup } = harness({
      executable: "outrim-test",
      spawnProcess: () => child,
    });
    const dispose = await cleanup;
    const structuredEvent = {
      status: "completed",
      tool: "custom",
      sessionID: "session-5",
      id: "structured",
      input: {},
      result: { output: { nested: ["must remain structured"] } },
    };
    await handlers.get("execute.after")(structuredEvent);
    expect(child.requests()).toHaveLength(0);

    const output = "original textual output";
    const textEvent = {
      status: "completed",
      tool: "shell",
      sessionID: "session-5",
      id: "text-output",
      input: { command: "cargo test" },
      result: { output },
    };
    const pending = handlers.get("execute.after")(textEvent);
    const [request] = child.requests();
    child.respond(request, "compressed textual output");
    await pending;

    expect(textEvent.result?.content).toBe("compressed textual output");
    expect(textEvent.result?.output).toBe(output);
    await dispose();
  });

  it("passes through when the bridge cannot start", async () => {
    const { handlers, cleanup } = harness({
      executable: "missing-outrim",
      spawnProcess() {
        throw new Error("missing executable");
      },
    });
    const dispose = await cleanup;
    const result = { content: [{ type: "text", text: "full original output" }] };
    const event = {
      status: "completed",
      tool: "shell",
      sessionID: "session-6",
      id: "call-6",
      input: { command: "cargo test" },
      result,
    };

    await handlers.get("execute.after")(event);

    expect(event.result).toBe(result);
    await dispose();
  });

  it("closes its child and releases pending results on plugin unload", async () => {
    const child = new FakeChild();
    const { handlers, cleanup } = harness({
      executable: "outrim-test",
      spawnProcess: () => child,
    });
    const dispose = await cleanup;
    const result = { content: [{ type: "text", text: "full original output" }] };
    const event = {
      status: "completed",
      tool: "shell",
      sessionID: "session-7",
      id: "call-7",
      input: { command: "cargo test" },
      result,
    };
    const pending = handlers.get("execute.after")(event);
    expect(child.requests()).toHaveLength(1);

    await dispose();
    await pending;

    expect(child.killed).toBe(true);
    expect(event.result).toBe(result);
  });

  it("passes through on session interruption", async () => {
    const child = new FakeChild();
    const { handlers, cleanup, emitEvent } = harness({
      executable: "outrim-test",
      spawnProcess: () => child,
    }, true);
    const dispose = await cleanup;
    await handlers.get("execute.before")({
      tool: "execute",
      sessionID: "session-4",
      id: "call-code-mode-4",
    });
    const result = { content: [{ type: "text", text: "original output" }] };
    const event = {
      status: "completed",
      tool: "shell",
      sessionID: "session-4",
      id: "call-4",
      input: { command: "cargo test" },
      result,
    };
    const pending = handlers.get("execute.after")(event);
    expect(child.requests()).toHaveLength(1);

    emitEvent({
      type: "session.execution.interrupted",
      properties: { sessionID: "session-4" },
    });
    await pending;

    expect(event.result).toBe(result);
    expect(child.killed).toBe(false);

    const afterInterruption = {
      status: "completed",
      tool: "shell",
      sessionID: "session-4",
      id: "call-code-mode-4",
      input: { command: "cargo test" },
      result: { content: [{ type: "text", text: "new session output" }] },
    };
    const afterInterruptionPending = handlers.get("execute.after")(afterInterruption);
    const [, newRequest] = child.requests();
    child.respond(newRequest, "compressed new-session output");
    await afterInterruptionPending;
    expect(afterInterruption.result.content).toEqual([
      { type: "text", text: "compressed new-session output" },
    ]);
    await dispose();
  });
});
