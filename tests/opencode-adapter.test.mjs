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
  killed = false;
  ended = false;
  autoAcknowledgeDeliveries = true;
  stdin = new EventEmitter();
  stdout = new EventEmitter();

  constructor() {
    super();
    this.stdin.write = (line, callback) => {
      this.writes.push(line);
      const request = JSON.parse(line);
      callback?.();
      if (request.type === "record_delivery" && this.autoAcknowledgeDeliveries) {
        this.stdout.emit("data", `${JSON.stringify({ version: 2, id: request.id })}\n`);
      }
      return true;
    };
    this.stdin.end = () => {
      this.ended = true;
    };
    this.stdout.setEncoding = () => {};
  }

  requests() {
    return this.writes
      .map((line) => JSON.parse(line))
      .filter((request) => request.type !== "record_delivery");
  }

  deliveryRequests() {
    return this.writes
      .map((line) => JSON.parse(line))
      .filter((request) => request.type === "record_delivery");
  }

  respond(request, text, metadata = {}) {
    this.stdout.emit(
      "data",
      `${JSON.stringify({
        version: 2,
        id: request.id,
        text,
        disposition: "compressed",
        applied_rule: "log_tail",
        ...metadata,
      })}\n`,
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
        signal.addEventListener(
          "abort",
          () => {
            if (!eventWaiter) return;
            const resolve = eventWaiter;
            eventWaiter = undefined;
            resolve({ done: true });
          },
          { once: true },
        );
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
        ["build", "adapters/opencode-plugin.ts", "--target=bun", `--outfile=${generatedAsset}`],
        { cwd: projectRoot, stdio: "ignore" },
      );
      const normalizeLineEndings = (text) => text.replace(/\r\n/g, "\n");
      expect(normalizeLineEndings(readFileSync(generatedAsset, "utf8"))).toBe(
        normalizeLineEndings(readFileSync(join(projectRoot, "assets/outrim-opencode.js"), "utf8")),
      );
    } finally {
      rmSync(tempDir, { force: true, recursive: true });
    }
  });

  it("exports the v2 plugin loader shape from the embedded asset", () => {
    expect(opencodePlugin.id).toBe("outrim");
    expect(typeof opencodePlugin.setup).toBe("function");
  });

  it.each([
    ["gh run watch", "runtime_log"],
    ["gh run watch 123 --exit-status", "runtime_log"],
    [" \tGH\tRUN WATCH 123", "runtime_log"],
    ["go test --workspace", "test_log"],
    ["go build ./...", "build_log"],
    ["npm run test -- --watch", "test_log"],
    ["gh run watcher", "unknown"],
    ['python -c "print(function_index)"', "unknown"],
    ['python3.12 -c "print(function_index)"', "unknown"],
    ['node -e "console.log(data)"', "unknown"],
    ["go version", "unknown"],
    ["npm run test-unit", "unknown"],
    ["npm run test:unit", "unknown"],
    ["jest.config.js", "unknown"],
    ["make-believe", "unknown"],
    ["tsc.exe", "unknown"],
  ])("uses the shared log allowlist: %s → %s", async (command, source) => {
    const child = new FakeChild();
    const { handlers, cleanup } = harness({
      executable: "outrim-test",
      spawnProcess: () => child,
    });
    const dispose = await cleanup;
    const event = {
      status: "completed",
      tool: "shell",
      sessionID: "session-1",
      id: "call-1",
      input: { command },
      result: { content: [{ type: "text", text: "original output" }] },
    };
    const pending = handlers.get("execute.after")(event);
    const [request] = child.requests();
    child.respond(request, "bridge-selected output");
    await pending;
    expect(request.tool).toMatchObject({ source, exit_code: null });
    expect(event.result.content[0].text).toBe("bridge-selected output");
    await dispose();
  });

  it.each([
    ...["shell", "bash", "powershell"].flatMap((tool) =>
      ["rg -n hit .", "grep -Rn hit .", "git grep -n hit", ' \tgit\tgrep -n "a|b;<>"'].map(
        (command) => [tool, command, "search"],
      ),
    ),
    ...["find", "grep", "ls", "glob", "list"].map((tool) => [tool, "", "search"]),
    ...["rg.exe hit", "git grepping hit", "/usr/bin/rg hit", "git -C repo grep hit"].map(
      (command) => ["shell", command, "unknown"],
    ),
  ])("classifies search events through %s: %s → %s", async (tool, command, source) => {
    const child = new FakeChild();
    const { handlers, cleanup } = harness({
      executable: "outrim-test",
      spawnProcess: () => child,
    });
    const dispose = await cleanup;
    const event = {
      status: "completed",
      tool,
      sessionID: "session-1",
      id: "call-1",
      input: { command },
      result: { content: [{ type: "text", text: "original output" }] },
    };
    const pending = handlers.get("execute.after")(event);
    const [request] = child.requests();
    child.respond(request, "original output");
    await pending;
    expect(request.tool.source).toBe(source);
    await dispose();
  });

  it.each([
    ...["shell", "bash", "powershell"].flatMap((tool) =>
      [
        "cargo test && git diff",
        "cargo test || cargo build",
        "cargo test; git diff",
        "cargo test\r\ngit diff",
        "cargo test | Select-Object -Last 2",
        "cargo test &",
        "cargo test $(git diff)",
        "cargo test `git diff`",
        "cargo test 'unclosed",
        'cargo test "unclosed',
        "& { cargo test; git diff }",
        "cargo test @('one', 'two')",
        'cargo test "a`"; git diff #"',
        'cargo test "a”; git diff #"',
        "powershell -Command 'cargo test'",
        "pwsh -c 'cargo test'",
        "bash -c 'cargo test'",
        'cmd /c "cargo test & git diff"',
        "Invoke-Expression 'cargo test'",
        "cargo test --% && git diff",
        '"grep" -n hit .',
        "env rg hit",
        "rg hit | sort",
        "grep hit; echo done",
        "git grep hit && echo done",
        'rg "$(echo hit)"',
        "rg 'unclosed",
        "",
        null,
      ].map((command) => [tool, command]),
    ),
    ...["cargo test 'a&git diff&rem '", 'cargo test "%SOME_VAR%"', 'cargo test "a\nb"'].map(
      (command) => ["shell", command],
    ),
  ])("preserves the whole result for %s complex or uncertain input: %s", async (tool, command) => {
    const child = new FakeChild();
    const { handlers, cleanup } = harness({ executable: "outrim-test", spawnProcess: () => child });
    const dispose = await cleanup;
    const text = `${Array.from({ length: 100 }, (_, i) => `工具 log ${i}\n`).join("")}diff 🦀`;
    const result = {
      content: [
        { type: "text", text, annotations: { retained: true } },
        { type: "image", data: "retained" },
        {
          type: "text",
          text: JSON.stringify(
            Array.from({ length: 100 }, () => ({ name: "same", value: 123 })),
            null,
            2,
          ),
        },
        {
          type: "text",
          text: Array.from({ length: 100 }, (_, i) => `src/main.rs:${i + 1}:same hit`).join("\n"),
        },
        { type: "text", text: "" },
      ],
      metadata: { exit: 1, retained: true },
      output: { retained: true },
    };
    const before = structuredClone(result);
    const event = {
      status: "completed",
      tool,
      sessionID: "session-1",
      id: "call-1",
      input: { command },
      result,
    };
    const pending = handlers.get("execute.after")(event);
    expect(child.requests().filter((request) => !request.type)).toEqual([]);
    await pending;
    expect(event.result).toBe(result);
    expect(event.result).toEqual(before);
    const count = result.content
      .filter((part) => part.type === "text")
      .reduce((total, part) => total + Array.from(part.text).length, 0);
    expect(child.deliveryRequests()).toHaveLength(1);
    expect(child.deliveryRequests()[0]).toMatchObject({
      disposition: "protected",
      applied_rule: null,
      before_chars: count,
      after_chars: count,
    });
    await dispose();
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
      version: 2,
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
    expect(child.deliveryRequests()).toHaveLength(1);
    expect(child.deliveryRequests()[0]).toMatchObject({
      disposition: "compressed",
      applied_rule: "log_tail",
      before_chars: Array.from("original output").length,
      after_chars: Array.from("compressed output").length,
    });
    await dispose();
  });

  it("records unchanged delivery outcomes without modifying the result", async () => {
    const child = new FakeChild();
    const { handlers, cleanup } = harness({
      executable: "outrim-test",
      spawnProcess: () => child,
    });
    const dispose = await cleanup;
    const result = { content: [{ type: "text", text: "protected output" }] };
    const event = {
      status: "completed",
      tool: "shell",
      sessionID: "session-protected",
      id: "call-protected",
      input: { command: "cargo check" },
      result,
    };

    const pending = handlers.get("execute.after")(event);
    const [request] = child.requests();
    child.respond(request, "protected output", {
      disposition: "protected",
      applied_rule: null,
    });
    await pending;

    expect(event.result).toBe(result);
    expect(child.deliveryRequests()[0]).toMatchObject({
      disposition: "protected",
      applied_rule: null,
      before_chars: Array.from("protected output").length,
      after_chars: Array.from("protected output").length,
    });
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
    const pendingNestedReadBeforeExecute = handlers.get("execute.after")(nestedReadBeforeExecute);
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

    expect(executeEventA.result.content).toEqual([{ type: "text", text: "compressed summary A" }]);
    expect(executeEventA.result.output).toBe(outputA);
    expect(executeEventB.result.content).toEqual([{ type: "text", text: "compressed summary B" }]);
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
        content: [
          { type: "text", text: "🦀 log output" },
          file,
          { type: "text", text: "🙂 unchanged" },
        ],
        output: structuredOutput,
        metadata: { exit: 0 },
      },
    };

    const pending = handlers.get("execute.after")(event);
    const [request, unchangedRequest] = child.requests();
    child.respond(request, "🦀 short");
    child.respond(unchangedRequest, "🙂 unchanged", {
      disposition: "no_match",
      applied_rule: null,
    });
    await pending;

    expect(event.result?.content).toEqual([
      { type: "text", text: "🦀 short" },
      file,
      { type: "text", text: "🙂 unchanged" },
    ]);
    expect(event.result?.content[1]).toBe(file);
    expect(event.result?.output).toBe(structuredOutput);
    expect(child.deliveryRequests()).toMatchObject([
      {
        disposition: "compressed",
        applied_rule: "log_tail",
        before_chars: Array.from("🦀 log output").length,
        after_chars: Array.from("🦀 short").length,
      },
      {
        disposition: "no_match",
        applied_rule: null,
        before_chars: Array.from("🙂 unchanged").length,
        after_chars: Array.from("🙂 unchanged").length,
      },
    ]);
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
      result: {
        content: [
          { type: "text", text: "🦀 recovered" },
          { type: "image", data: "retained" },
          { type: "text", text: "🙂 original" },
        ],
      },
    };
    await handlers.get("execute.after")(recovered);
    expect(child.requests()).toHaveLength(0);
    expect(child.deliveryRequests()).toMatchObject([
      {
        disposition: "recovery_skipped",
        applied_rule: null,
        before_chars: Array.from("🦀 recovered🙂 original").length,
        after_chars: Array.from("🦀 recovered🙂 original").length,
      },
    ]);

    const outputMentionsPath = {
      ...recovered,
      id: "normal",
      input: { command: "cargo test" },
      result: {
        content: [{ type: "text", text: "large output mentioning ~/.outrim/outputs/file.txt" }],
      },
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

    expect(child.ended).toBe(true);
    expect(child.killed).toBe(false);
    expect(event.result).toBe(result);
  });

  it("clears only the interrupted session's Code Mode state", async () => {
    const child = new FakeChild();
    const { handlers, cleanup, emitEvent } = harness(
      {
        executable: "outrim-test",
        spawnProcess: () => child,
      },
      true,
    );
    const dispose = await cleanup;
    const codeModeID = "shared-code-mode-call";
    await handlers.get("execute.before")({
      tool: "execute",
      sessionID: "session-a",
      id: codeModeID,
    });
    await handlers.get("execute.before")({
      tool: "execute",
      sessionID: "session-b",
      id: codeModeID,
    });

    const activeEvent = {
      status: "completed",
      tool: "shell",
      sessionID: "session-a",
      id: "active-shell-a",
      input: { command: "cargo test" },
      result: { content: [{ type: "text", text: "interrupted output" }] },
    };
    const activePending = handlers.get("execute.after")(activeEvent);
    expect(child.requests()).toHaveLength(1);
    emitEvent({
      type: "session.execution.interrupted",
      properties: { sessionID: "session-a" },
    });
    await activePending;

    const nestedResult = {
      content: [{ type: "text", text: "raw session B result" }],
      output: { rows: ["data still used by session B"] },
    };
    const nestedEvent = {
      status: "completed",
      tool: "read",
      sessionID: "session-b",
      id: codeModeID,
      input: { filePath: "src/main.rs" },
      result: nestedResult,
    };
    const nestedPending = handlers.get("execute.after")(nestedEvent);
    const requestsAfterNested = child.requests();
    for (const request of requestsAfterNested.slice(1)) {
      child.respond(request, "unexpected compression");
    }
    await nestedPending;
    expect(requestsAfterNested).toHaveLength(1);
    expect(nestedEvent.result).toBe(nestedResult);
    await dispose();
  });

  it("records fail-open after a bridge exit and restarts for later calls", async () => {
    const children = [];
    const { handlers, cleanup } = harness({
      executable: "outrim-test",
      spawnProcess() {
        const child = new FakeChild();
        children.push(child);
        return child;
      },
    });
    const dispose = await cleanup;
    const original = "output before bridge exit";
    const event = {
      status: "completed",
      tool: "shell",
      sessionID: "session-restart",
      id: "call-before-exit",
      input: { command: "cargo test" },
      result: { content: [{ type: "text", text: original }] },
    };

    const pending = handlers.get("execute.after")(event);
    children[0].exit();
    await pending;

    expect(event.result.content[0].text).toBe(original);
    expect(children).toHaveLength(2);
    expect(children[1].deliveryRequests()[0]).toMatchObject({
      disposition: "failed_open",
      applied_rule: null,
      before_chars: Array.from(original).length,
      after_chars: Array.from(original).length,
    });

    const nextEvent = {
      ...event,
      id: "call-after-exit",
      result: { content: [{ type: "text", text: "later output" }] },
    };
    const nextPending = handlers.get("execute.after")(nextEvent);
    const [nextRequest] = children[1].requests();
    children[1].respond(nextRequest, "compressed later output");
    await nextPending;
    expect(nextEvent.result.content[0].text).toBe("compressed later output");
    await dispose();
  });

  it("passes through on session interruption", async () => {
    const child = new FakeChild();
    const { handlers, cleanup, emitEvent } = harness(
      {
        executable: "outrim-test",
        spawnProcess: () => child,
      },
      true,
    );
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
