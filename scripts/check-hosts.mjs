import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const original = Array.from(
  { length: 100 },
  (_, index) => `🦀 log ${index}: ${"工具 output ".repeat(20)}\n`,
).join("");
const chars = (text) => Array.from(text).length;

async function checkHost(host) {
  const handlers = new Map();
  let handle;
  let close;
  if (host === "pi") {
    const { default: setup } = await import(
      pathToFileURL(join(process.env.PI_CODING_AGENT_DIR, "extensions/outrim.js")).href
    );
    setup({ on: (name, handler) => handlers.set(name, handler) });
    handle = async (text, input) => {
      const event = {
        toolName: "bash",
        toolCallId: "call",
        input,
        content: [{ type: "text", text }],
        structuredContent: { exit_code: 0 },
        isError: false,
      };
      const result = await handlers.get("tool_result")(event, {
        sessionManager: { getSessionId: () => "roundtrip" },
      });
      return (result ?? event).content[0].text;
    };
    close = () => handlers.get("session_shutdown")();
  } else {
    const { default: plugin } = await import(
      pathToFileURL(join(process.env.XDG_CONFIG_HOME, "opencode/plugins/outrim.js")).href
    );
    close = await plugin.setup({
      tool: {
        hook: async (name, handler) => {
          handlers.set(name, handler);
          return { dispose() {} };
        },
      },
    });
    handle = async (text, input) => {
      const image = { type: "image", data: "retained" };
      const metadata = { exit: 0 };
      const event = {
        status: "completed",
        tool: "shell",
        sessionID: "roundtrip",
        id: "call",
        input,
        result: { content: [{ type: "text", text }, image], metadata },
      };
      await handlers.get("execute.after")(event);
      assert.equal(event.result.content[1], image);
      assert.equal(event.result.metadata, metadata);
      return event.result.content[0].text;
    };
  }

  try {
    const compressed = await handle(original, { command: "cargo test" });
    assert.ok(chars(compressed) < chars(original));
    const path = compressed.split("[outrim]: The full output is available at ")[1];
    assert.ok(path, "compressed output must expose its saved original");
    assert.equal(readFileSync(path, "utf8"), original);
    assert.equal(await handle(original, { path }), original, "recovery must bypass compression");
    return compressed;
  } finally {
    await close();
  }
}

if (process.argv[2] === "--host") {
  // The parent waits for this process (including the real bridge) to exit.
  // Closing stdin drains delivery records; no sleep or acknowledgement assumption is needed.
  console.log(JSON.stringify(await checkHost(process.argv[3])));
} else {
  const build = execFileSync("cargo", ["build", "--locked", "--message-format=json"], {
    cwd: root,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "inherit"],
  });
  const artifact = build
    .trim()
    .split("\n")
    .map((line) => JSON.parse(line))
    .find((message) => message.target?.name === "outrim" && message.executable);
  assert.ok(artifact?.executable, "Cargo must produce the outrim executable");

  for (const host of ["pi", "opencode"]) {
    const home = mkdtempSync(join(tmpdir(), `outrim-${host}-roundtrip-`));
    const env = {
      ...process.env,
      HOME: home,
      USERPROFILE: home,
      PI_CODING_AGENT_DIR: join(home, "pi"),
      XDG_CONFIG_HOME: join(home, "config"),
    };
    try {
      execFileSync(artifact.executable, ["install", "--agent", host], { env });
      const compressed = JSON.parse(
        execFileSync(process.execPath, [import.meta.filename, "--host", host], {
          env,
          encoding: "utf8",
          timeout: 30_000,
        }),
      );
      const stats = JSON.parse(readFileSync(join(home, ".outrim/gain.json"), "utf8"));
      assert.equal(stats.outputs, 2);
      assert.equal(stats.outcomes.compressed, 1);
      assert.equal(stats.outcomes.recovery_skipped, 1);
      assert.equal(stats.before_chars, chars(original) * 2);
      assert.equal(stats.after_chars, chars(compressed) + chars(original));
      assert.deepEqual(stats.rules, {
        log_tail: { count: 1, before_chars: chars(original), after_chars: chars(compressed) },
      });
      const gain = execFileSync(artifact.executable, ["gain"], { env, encoding: "utf8" });
      assert.match(gain, /Outputs: 2\b/);
      console.log(`${host}: installed bundle → Rust → original/recovery/gain passed`);
    } finally {
      rmSync(home, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
    }
  }
}
