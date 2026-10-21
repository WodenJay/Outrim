import { describe, expect, it } from "bun:test";
import { EventEmitter } from "node:events";
import { registerPiExtension } from "../assets/outrim-pi.js";
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
      callback?.();
      const request = JSON.parse(line);
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
    return this.writes.map((line) => JSON.parse(line));
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
  it("preserves a compound log command's entire result", async () => {
    const { children, handle } = harness();
    const text = `${Array.from({ length: 100 }, (_, i) => `工具 log ${i}\n`).join("")}diff --git a/x b/x`;
    const event = toolEvent({
      input: { command: "cargo test && git diff" },
      content: [{ type: "text", text }],
    });
    const before = structuredClone(event);
    const pending = handle(event);
    const requests = children.flatMap((child) => child.requests());
    expect(requests.filter((request) => !request.type)).toEqual([]);
    expect(await pending).toBeUndefined();
    expect(event).toEqual(before);
    expect(requests.find((request) => request.type === "record_delivery")).toMatchObject({
      disposition: "protected",
      applied_rule: null,
      before_chars: Array.from(text).length,
      after_chars: Array.from(text).length,
    });
  });
  it.each(
    ["bash", "powershell"].flatMap((toolName) =>
      ["rg -n hit .", "grep -Rn hit .", "git grep -n hit", " \trg -n 'a|b' ."].map((command) => [
        toolName,
        command,
      ]),
    ),
  )("recognizes direct search through %s: %s", async (toolName, command) => {
    const { children, handle } = harness();
    const pending = handle(toolEvent({ toolName, input: { command } }));
    const [request] = children[0].requests();
    children[0].respond(request, "original output");
    expect(request.tool.source).toBe("search");
    await pending;
  });
  it.each([
    "rg-extra -n hit .",
    "grep.exe -n hit .",
    "git grepping hit",
    "/usr/bin/rg -n hit .",
    "git -C repo grep -n hit",
  ])("keeps unsupported simple search spellings unknown: %s", async (command) => {
    const { children, handle } = harness();
    const pending = handle(toolEvent({ input: { command } }));
    const [request] = children[0].requests();
    children[0].respond(request, "original output");
    expect(request.tool.source).toBe("unknown");
    await pending;
  });

  it.each([
    "rg\u00a0-n hit .",
    "git\u00a0grep hit",
    "rg\f-n hit .",
    '"rg" -n hit .',
    "env rg -n hit .",
    "FOO=1 rg -n hit .",
    "sudo grep -n hit .",
    "rg -n hit . | sort",
    "grep -n hit . || echo missing",
    "git grep -n hit && echo done",
    "rg -n hit .; echo done",
    "rg -n hit .\necho done",
    "rg -n hit . > hits.txt",
    "rg -n $(echo hit) .",
    'rg -n "$(echo hit)" .',
    "rg -n `echo hit` .",
    "rg -n $pattern .",
    "rg -n hit . # comment",
    "rg -n 'unclosed",
    'rg -n "unclosed',
    String.raw`rg -n \"hit | sort`,
    "cargo test && cargo build",
    "cd project && cargo test",
    "cargo test || git diff",
    "cargo test; git diff",
    "cargo test\ngit diff",
    "cargo test &",
    "cargo test $(git diff)",
    "cargo test <(git diff)",
    "bash -c 'cargo test'",
    "cargo test \\\n git diff",
    null,
  ])("preserves all text for compound or uncertain Bash: %s", async (command) => {
    const { children, handle } = harness();
    const outputs = [
      `${Array.from({ length: 100 }, (_, i) => `工具 log ${i}\r\n`).join("")}diff 🦀`,
      JSON.stringify(
        Array.from({ length: 100 }, () => ({ name: "same", value: 123 })),
        null,
        2,
      ),
      Array.from({ length: 100 }, (_, i) => `src/main.rs:${i + 1}:same hit`).join("\n"),
      "",
    ];
    for (const text of outputs) {
      const event = toolEvent({
        input: { command },
        content: [{ type: "text", text, extra: true }],
      });
      const before = structuredClone(event);
      const pending = handle(event);
      expect(
        children.flatMap((child) => child.requests()).filter((request) => !request.type),
      ).toEqual([]);
      expect(await pending).toBeUndefined();
      expect(event).toEqual(before);
    }
  });

  it.each(
    ["bash", "powershell"].flatMap((toolName) =>
      [
        'cargo test "a&&b;|<>()"',
        "cargo test 'a&&b;|<>()'",
        "cargo test 'a''b&&c'",
        "cargo test '$literal `literal \\literal'",
      ].map((command) => [toolName, command]),
    ),
  )(
    "retains simple log eligibility with literal quoted syntax in %s: %s",
    async (toolName, command) => {
      const { children, handle } = harness();
      const pending = handle(toolEvent({ toolName, input: { command } }));
      const [request] = children[0].requests();
      expect(request.tool.source).toBe("test_log");
      children[0].respond(request, "last ten lines");
      expect((await pending).content[0].text).toBe("last ten lines");
    },
  );

  it.each(["find", "grep", "ls"])("preserves native %s search classification", async (toolName) => {
    const { children, handle } = harness();
    const pending = handle(toolEvent({ toolName, input: { pattern: "hit" } }));
    const [request] = children[0].requests();
    children[0].respond(request, "original output");
    expect(request.tool.source).toBe("search");
    await pending;
  });

  it.each([
    'python -c "print(function_index)"',
    'python3 -c "print(function_index)"',
    'python3.12 -c "print(function_index)"',
    'node -e "console.log(data)"',
    "go version",
    "go run main.go",
  ])("does not infer runtime logs from the interpreter: %s", async (command) => {
    const { children, handle } = harness();
    const pending = handle(toolEvent({ input: { command } }));
    const [request] = children[0].requests();
    children[0].respond(request, "original output", {
      disposition: "no_match",
      applied_rule: null,
    });
    expect(request.tool.source).toBe("unknown");
    expect(await pending).toBeUndefined();
  });

  it.each([
    ...[
      ["test_log", ["cargo test", "pytest", "go test", "jest", "vitest"]],
      [
        "build_log",
        [
          "cargo build",
          "cargo check",
          "cargo clippy",
          "go build",
          "tsc",
          "vite build",
          "cmake --build",
          "make",
        ],
      ],
      [
        "runtime_log",
        ["cargo run", "gh run watch", "gh run watch 123", "gh run watch 123 --exit-status"],
      ],
      ...["bun", "npm", "pnpm", "yarn"].flatMap((manager) => [
        ["test_log", [`${manager} test`, `${manager} run test`]],
        ["build_log", [`${manager} build`, `${manager} run build`]],
        ["runtime_log", [`${manager} start`, `${manager} run dev`, `${manager} run start`]],
      ]),
    ].flatMap(([source, commands]) =>
      commands.flatMap((command) => [
        [command, source],
        [` \t${command.toUpperCase().replaceAll(" ", "\t ")} --verbose`, source],
      ]),
    ),
    ...[
      "npm run test-unit",
      "npm run test:unit",
      "go test-unit",
      "jest.config.js",
      "make-believe",
      "gh run watcher",
      "tsc.exe",
      "cargo build-all",
      "npm run dev:local",
      "/usr/bin/pytest",
      "npx vitest",
      "npm --prefix app test",
      "gh --repo owner/repo run watch",
      "python -m pytest",
    ].map((command) => [command, "unknown"]),
  ])("classifies complete allowlist words: %s → %s", async (command, source) => {
    const { children, handle } = harness();
    const pending = handle(toolEvent({ input: { command } }));
    const [request] = children[0].requests();
    children[0].respond(request, "original output");
    expect(request.tool.source).toBe(source);
    expect(await pending).toBeUndefined();
  });

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
      version: 2,
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
    const recoveryRecord = children[0].requests()[0];
    expect(recoveryRecord).toMatchObject({
      type: "record_delivery",
      disposition: "recovery_skipped",
      applied_rule: null,
      before_chars: "original output".length,
      after_chars: "original output".length,
    });
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
    expect(children).toHaveLength(1);
    expect(
      children[0].requests().filter((request) => request.type === "record_delivery"),
    ).toHaveLength(7);

    const outputMentionsDirectory = handle(
      toolEvent({
        input: { command: "cargo test" },
        content: [{ type: "text", text: "output mentions ~/.outrim/outputs/saved.txt" }],
      }),
    );
    const request = children[0].requests().find((request) => !request.type);
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
    expect(
      children[0].requests().filter((request) => request.type === "record_delivery"),
    ).toHaveLength(1);
    expect(children).toHaveLength(1);
  });

  it("records an adopted compressed result once with Unicode codepoint lengths", async () => {
    const { children, handle } = harness();
    const original = "🦀🙂\n".repeat(12);
    const pending = handle(toolEvent({ content: [{ type: "text", text: original }] }));
    const processRequest = children[0].requests()[0];
    children[0].respond(processRequest, "short 🦀", {
      disposition: "compressed",
      applied_rule: "log_tail",
    });

    const result = await pending;
    expect(result.content[0].text).toBe("short 🦀");
    expect(children[0].requests()).toHaveLength(2);
    expect(children[0].requests()[1]).toMatchObject({
      type: "record_delivery",
      disposition: "compressed",
      applied_rule: "log_tail",
      before_chars: Array.from(original).length,
      after_chars: Array.from("short 🦀").length,
    });
  });

  it("records unchanged outcomes without changing the host result", async () => {
    const { children, handle } = harness();
    const original = "kept as-is";
    const pending = handle(toolEvent({ content: [{ type: "text", text: original }] }));
    const request = children[0].requests()[0];
    children[0].respond(request, original, {
      disposition: "protected",
      applied_rule: null,
    });

    expect(await pending).toBeUndefined();
    expect(children[0].requests()[1]).toMatchObject({
      type: "record_delivery",
      disposition: "protected",
      applied_rule: null,
      before_chars: original.length,
      after_chars: original.length,
    });
  });

  it("records fail-open after child exit and reuses the restarted child", async () => {
    const { children, handle } = harness();
    const pending = handle(toolEvent());
    children[0].exit();
    expect(await pending).toBeUndefined();
    expect(children).toHaveLength(2);
    expect(children[1].requests()[0]).toMatchObject({
      type: "record_delivery",
      disposition: "failed_open",
    });

    const restarted = handle(toolEvent({ toolCallId: "call-after-exit" }));
    expect(children).toHaveLength(2);
    const request = children[1].requests().find((request) => !request.type);
    children[1].respond(request, "restarted result");
    expect((await restarted).content[0].text).toBe("restarted result");
  });

  it("drains a queued delivery when closing the session", async () => {
    const { handlers, children, handle } = harness();
    const pending = handle(toolEvent());
    const child = children[0];
    child.autoAcknowledgeDeliveries = false;
    const processRequest = child.requests()[0];
    child.respond(processRequest, "compressed output");

    expect((await pending).content[0].text).toBe("compressed output");
    expect(child.requests()[1].type).toBe("record_delivery");

    handlers.get("session_shutdown")();
    expect(child.ended).toBe(true);
    expect(child.killed).toBe(false);
  });

  it("closes its child input and resolves pending work on session shutdown", async () => {
    const { handlers, children, handle } = harness();
    const pending = handle(toolEvent());
    const child = children[0];

    handlers.get("session_shutdown")();
    expect(await pending).toBeUndefined();
    expect(child.ended).toBe(true);
    expect(child.killed).toBe(false);
    expect(children).toHaveLength(1);
  });
});
