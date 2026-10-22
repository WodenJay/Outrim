// Installed OpenCode adapter + real bridge + native shell. This is not a real host run.
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, readdirSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = dirname(dirname(fileURLToPath(import.meta.url)));

if (process.argv[2] === "--host") {
  const { default: plugin } = await import(
    pathToFileURL(join(process.env.XDG_CONFIG_HOME, "opencode/plugins/outrim.js")).href
  );
  const handlers = new Map();
  const close = await plugin.setup(
    Object.fromEntries(
      ["tool", "session", "shell"].map((domain) => [
        domain,
        {
          hook: async (name, handler) => {
            handlers.set(`${domain}.${name}`, handler);
            return { dispose() {} };
          },
        },
      ]),
    ),
  );
  const recording = process.env.OUTRIM_CHECK_RECORDING === "on";
  // The snapshot must survive a settings change before the bridge even starts.
  execFileSync(process.env.OUTRIM_BINARY, ["gain", "record", recording ? "off" : "on"]);
  try {
    handlers.get("session.context")({
      sessionID: "roundtrip",
      agent: "build",
      tools: {
        shell: {
          description:
            "Execute a shell command and return its output. Commands run on Linux using bash.",
        },
      },
    });
    const path = process.env.OUTRIM_FIXTURE.replaceAll("\\", "/");
    const event = {
      tool: "shell",
      sessionID: "roundtrip",
      agent: "build",
      id: "wrapped",
      input: { command: `'${path}' test --report` },
    };
    await handlers.get("tool.execute.before")(event);
    assert.ok(event.input.command.includes("exec --context"));
    const invocation = { shell: "bash", command: event.input.command };
    handlers.get("shell.create.before")(invocation);
    const output = spawnSync("bash", ["-c", invocation.command], { encoding: "utf8" });
    assert.equal(output.status, 19, output.stderr);
    const originals = Object.entries({ stdout: output.stdout, stderr: output.stderr }).map(
      ([stream, text]) => {
        const path = text.split("The full output is available at ")[1]?.trim();
        assert.ok(path, text);
        const original = readFileSync(path, "utf8");
        assert.ok(original.includes("running 100 tests"));
        assert.ok(original.includes(`test ${stream}_case_99 ... ok`));
        assert.equal(original.includes(stream === "stdout" ? "stderr_case" : "stdout_case"), false);
        return path;
      },
    );
    assert.equal(readFileSync("executed", "utf8"), "once");
    const result = {
      content: [{ type: "text", text: output.stdout + output.stderr }],
      metadata: { exit: 19 },
    };
    const finished = { ...event, status: "completed", result };
    await handlers.get("tool.execute.after")(finished);
    assert.equal(finished.result, result);
    for (const command of [
      "cargo test | sort",
      "outrim exec -- cargo test",
      "outrim exec --no-compress -- cargo test",
    ]) {
      const input = { command };
      await handlers.get("tool.execute.before")({ ...event, input });
      assert.equal(input.command, command);
      await handlers.get("tool.execute.after")({ ...finished, input });
    }
    const original = readFileSync(originals[0], "utf8");
    const recovered = {
      ...finished,
      tool: "read",
      id: "recovery",
      input: { filePath: originals[0] },
      result: { content: [{ type: "text", text: original }] },
    };
    await handlers.get("tool.execute.after")(recovered);
    assert.equal(recovered.result.content[0].text, original);
    const json = JSON.stringify(
      Array.from({ length: 100 }, () => ({ name: "工具", value: 123 })),
      null,
      2,
    );
    const image = { type: "image", data: "retained" };
    const metadata = { exit: 0 };
    const structured = { rows: ["caller data"] };
    const native = {
      ...finished,
      tool: "custom",
      id: "native",
      input: {},
      result: { content: [{ type: "text", text: json }, image], metadata, output: structured },
    };
    await handlers.get("tool.execute.after")(native);
    assert.ok(native.result.content[0].text.length < json.length);
    assert.equal(native.result.content[1], image);
    assert.equal(native.result.metadata, metadata);
    assert.equal(native.result.output, structured);
  } finally {
    await close();
  }
} else {
  const build = execFileSync("cargo", ["build", "--locked", "--message-format=json"], {
    cwd: root,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "inherit"],
  });
  const artifact = build
    .trim()
    .split("\n")
    .map(JSON.parse)
    .find((message) => message.target?.name === "outrim" && message.executable);
  assert.ok(artifact?.executable, "Cargo must produce the outrim executable");
  execFileSync(process.execPath, [join(root, "scripts/check-pi.mjs"), artifact.executable], {
    stdio: "inherit",
  });

  for (const recording of [false, true]) {
    const home = mkdtempSync(join(tmpdir(), "outrim-opencode-roundtrip-"));
    const fixture = join(home, process.platform === "win32" ? "cargo.exe" : "cargo");
    const env = {
      ...process.env,
      HOME: home,
      USERPROFILE: home,
      XDG_CONFIG_HOME: join(home, "config"),
      OUTRIM_BINARY: artifact.executable,
      OUTRIM_FIXTURE: fixture,
      OUTRIM_CHECK_RECORDING: recording ? "on" : "off",
    };
    try {
      execFileSync("rustc", [
        "--edition=2024",
        join(root, "tests/fixtures/exec-child.rs"),
        "-o",
        fixture,
      ]);
      execFileSync(artifact.executable, ["install", "--agent", "opencode"], { env });
      execFileSync(artifact.executable, ["gain", "record", recording ? "on" : "off"], { env });
      execFileSync(process.execPath, [import.meta.filename, "--host"], {
        env,
        cwd: home,
        stdio: "inherit",
        timeout: 30_000,
      });
      const stats = JSON.parse(readFileSync(join(home, ".outrim/gain.json"), "utf8"));
      assert.equal(
        Object.values(stats.outcomes).reduce((total, count) => total + count, 0),
        4,
        "two streams + recovery read + native tool; no shell-result samples",
      );
      assert.equal(stats.outcomes.compressed, 3);
      assert.equal(stats.outcomes.recovery_skipped, 1);
      assert.ok(stats.recovery_chars > 0);
      if (!recording) {
        assert.throws(() => readdirSync(join(home, ".outrim/records")), { code: "ENOENT" });
        continue;
      }
      const events = readdirSync(join(home, ".outrim/records")).flatMap((instance) =>
        readFileSync(join(home, ".outrim/records", instance, "events.jsonl"), "utf8")
          .trim()
          .split("\n")
          .map(JSON.parse),
      );
      const streams = events.filter((event) => event.tool_call_id === "wrapped" && event.execution);
      assert.deepEqual(streams.map((event) => event.execution.stream).sort(), ["stderr", "stdout"]);
      for (const event of streams) {
        assert.equal(event.host, "opencode");
        assert.equal(event.session, "roundtrip");
        assert.equal(event.execution.unit_id, "wrapped:0");
        assert.equal(event.execution.recording, true);
      }
      const recovered = events.filter(
        (event) => event.outcome === "recovery_skipped" && !event.metadata_only,
      );
      assert.equal(recovered.length, 1);
      const stdout = streams.find((event) => event.execution.stream === "stdout");
      assert.deepEqual(recovered[0].original_references, [
        { original_path: stdout.original_path, record_id: stdout.event_id },
      ]);
    } finally {
      rmSync(home, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
    }
  }
  console.log(
    "opencode: installed bundle → rewrite → native shell → per-stream originals/gain/records passed",
  );
}
