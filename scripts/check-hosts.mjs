import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readdirSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const original = Array.from(
  { length: 100 },
  (_, index) => `🦀 log ${index}: ${"工具 output ".repeat(20)}\n`,
).join("");
const chars = (text) => Array.from(text).length;
const passthroughCases = [
  ["cargo test && git diff", original],
  ["cargo fmt --all && cargo clippy && cargo test", original],
  ["cd project && cargo test", original],
  [
    "cargo test | sort",
    JSON.stringify(
      Array.from({ length: 100 }, () => ({ name: "same", value: 123 })),
      null,
      2,
    ),
  ],
  [
    "rg hit .; echo done",
    Array.from({ length: 100 }, (_, i) => `src/main.rs:${i + 1}:same hit`).join("\n"),
  ],
  ["cargo test $(git diff)", ""],
  ["cargo test\ngit diff", "Unicode 🦀\r\nno final newline"],
];
const passthroughChars = passthroughCases.reduce((total, [, text]) => total + chars(text), 0);

async function checkHost() {
  const handlers = new Map();
  let handle;
  let close;
  let unsupported;
  let multipart;
  {
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
    multipart = async () => {
      const event = {
        status: "completed",
        tool: "shell",
        sessionID: "roundtrip",
        id: "parts",
        input: { command: "cargo test" },
        result: { content: [original, "small", original].map((text) => ({ type: "text", text })) },
      };
      await handlers.get("execute.after")(event);
      const path = event.result.content[0].text.split(
        "[outrim]: The full output is available at ",
      )[1];
      assert.ok(path);
      await handle("reference result", { path });
    };
    unsupported = () =>
      handlers.get("execute.after")({
        status: "error",
        tool: "shell",
        sessionID: "roundtrip",
        id: "unsupported",
        input: { command: "echo hello" },
        result: { content: [{ type: "text", text: "visible 🦀" }] },
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
    // A changed setting must not affect this already-loaded host, even before
    // its first bridge starts. Child bridges use the host's startup snapshot.
    if (process.env.OUTRIM_CHECK_RECORDING === "on") {
      execFileSync(process.env.OUTRIM_BINARY, ["gain", "record", "off"]);
    }
    const compressed = await handle(original, { command: "cargo test" });
    assert.ok(chars(compressed) < chars(original));
    assert.ok(compressed.startsWith(original.slice(original.indexOf("🦀 log 90:"))));
    const path = compressed.split("[outrim]: The full output is available at ")[1];
    assert.ok(path, "compressed output must expose its saved original");
    assert.equal(readFileSync(path, "utf8"), original);
    assert.equal(await handle(original, { path }), original, "recovery must bypass compression");
    if (process.env.OUTRIM_CHECK_RECORDING === "on") {
      assert.equal(await handle("unchanged 🦀", { command: "echo hello" }), "unchanged 🦀");
      await unsupported();
      await multipart?.();
    }
    const outputsDirectory = dirname(path);
    const outputsBefore = readdirSync(outputsDirectory, { recursive: true });
    for (const [command, text] of passthroughCases) {
      assert.equal(await handle(text, { command }), text, `must preserve ${command}`);
    }
    assert.deepEqual(
      readdirSync(outputsDirectory, { recursive: true }),
      outputsBefore,
      "passthrough must not save compression originals",
    );
    return compressed;
  } finally {
    await close();
  }
}

if (process.argv[2] === "--host") {
  // The parent waits for this process (including the real bridge) to exit.
  // Closing stdin drains delivery records; no sleep or acknowledgement assumption is needed.
  console.log(JSON.stringify(await checkHost()));
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

  execFileSync(process.execPath, [join(root, "scripts/check-pi.mjs"), artifact.executable], {
    stdio: "inherit",
  });

  for (const host of ["opencode"]) {
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
      const runHost = () =>
        JSON.parse(
          execFileSync(process.execPath, [import.meta.filename, "--host", host], {
            env,
            encoding: "utf8",
            timeout: 30_000,
          }),
        );
      const compressed = runHost();
      assert.throws(() => readdirSync(join(home, ".outrim/records")), { code: "ENOENT" });
      const stats = JSON.parse(readFileSync(join(home, ".outrim/gain.json"), "utf8"));
      assert.equal(stats.outputs, 2 + passthroughCases.length);
      assert.equal(stats.outcomes.protected, passthroughCases.length);
      assert.equal(stats.outcomes.compressed, 1);
      assert.equal(stats.outcomes.recovery_skipped, 1);
      assert.equal(stats.before_chars, chars(original) * 2 + passthroughChars);
      assert.equal(stats.after_chars, chars(compressed) + chars(original) + passthroughChars);
      assert.deepEqual(stats.rules, {
        log_tail: { count: 1, before_chars: chars(original), after_chars: chars(compressed) },
      });
      const gain = execFileSync(artifact.executable, ["gain"], { env, encoding: "utf8" });
      assert.ok(gain.includes(`Outputs: ${2 + passthroughCases.length}`));
      execFileSync(artifact.executable, ["gain", "record", "on"], { env });
      env.OUTRIM_CHECK_RECORDING = "on";
      env.OUTRIM_BINARY = artifact.executable;
      const recordedFinal = runHost();
      const records = join(home, ".outrim/records");
      const events = readdirSync(records).flatMap((instance) => {
        const directory = join(records, instance);
        return readFileSync(join(directory, "events.jsonl"), "utf8")
          .trim()
          .split("\n")
          .map((line) => ({ ...JSON.parse(line), directory }));
      });
      assert.equal(events.length, 6 + passthroughCases.length);
      for (const [index, event] of events.slice(-passthroughCases.length).entries()) {
        const [, text] = passthroughCases[index];
        assert.equal(event.outcome, "protected");
        assert.equal(event.reason, "compound_or_uncertain_shell_command");
        assert.equal(event.applied_rule, null);
        assert.equal(event.original_path, null);
        assert.deepEqual(event.candidates, []);
        assert.equal(event.before_chars, chars(text));
        assert.equal(event.after_chars, chars(text));
        assert.equal(event.content.original, event.content.final);
        assert.equal(readFileSync(join(event.directory, event.content.original), "utf8"), text);
      }
      const [compressedEvent, recovery] = events;
      assert.equal(compressedEvent.host, host);
      assert.equal(compressedEvent.session, "roundtrip");
      assert.equal(compressedEvent.tool_call_id, "call");
      assert.equal(compressedEvent.outcome, "compressed");
      assert.equal(compressedEvent.applied_rule, "log_tail");
      assert.ok(compressedEvent.candidates.some((candidate) => candidate.rule === "log_tail"));
      assert.deepEqual(
        JSON.parse(
          readFileSync(join(compressedEvent.directory, compressedEvent.content.arguments), "utf8"),
        ),
        { command: "cargo test" },
      );
      assert.equal(
        readFileSync(join(compressedEvent.directory, compressedEvent.content.original), "utf8"),
        original,
      );
      assert.equal(
        readFileSync(join(compressedEvent.directory, compressedEvent.content.final), "utf8"),
        recordedFinal,
      );
      assert.equal(compressedEvent.before_chars, chars(original));
      assert.equal(compressedEvent.after_chars, chars(recordedFinal));
      assert.equal(recovery.outcome, "recovery_skipped");
      assert.deepEqual(recovery.original_references, [
        {
          original_path: compressedEvent.original_path,
          record_id: compressedEvent.event_id,
        },
      ]);
      assert.equal(
        readFileSync(join(recovery.directory, recovery.content.final), "utf8"),
        original,
      );
      const [, , unchanged, skipped] = events;
      assert.equal(unchanged.outcome, "no_match");
      assert.equal(
        readFileSync(join(unchanged.directory, unchanged.content.original), "utf8"),
        "unchanged 🦀",
      );
      assert.equal(unchanged.content.original, unchanged.content.final);
      assert.equal(skipped.outcome, "unsupported");
      assert.ok(skipped.reason);
      assert.equal(skipped.tool_call_id, "unsupported");
      assert.equal(
        readFileSync(join(skipped.directory, skipped.content.original), "utf8"),
        "visible 🦀",
      );
      if (host === "opencode") {
        const [parts, reference] = events.slice(4);
        assert.equal(parts.outcome, "mixed");
        assert.equal(parts.original_path, null);
        assert.deepEqual(
          parts.parts.map((part) => part.outcome),
          ["compressed", "not_shorter", "compressed"],
        );
        assert.equal(reference.original_references[0].original_path, parts.parts[0].original_path);
        assert.equal(reference.original_references[0].record_id, parts.event_id);
      }
      // The running host kept recording after off; a new host must not append.
      delete env.OUTRIM_CHECK_RECORDING;
      runHost();
      assert.equal(readdirSync(records).length, 1);
      assert.equal(
        readFileSync(join(compressedEvent.directory, "events.jsonl"), "utf8").trim().split("\n")
          .length,
        events.length,
      );
      console.log(`${host}: installed bundle → Rust → original/recovery/gain/recording passed`);
    } finally {
      rmSync(home, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
    }
  }
}
