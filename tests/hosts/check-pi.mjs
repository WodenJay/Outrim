// Installed Pi adapter + real bridge + native shell. SDK dispatch is verified separately.
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

if (process.argv[2] === "--host") {
  const { registerPiExtension } = await import(
    pathToFileURL(join(process.env.PI_CODING_AGENT_DIR, "extensions/outrim.js")).href
  );
  const handlers = new Map();
  registerPiExtension({ on: (name, handler) => handlers.set(name, handler) }, { dialect: "bash" });
  const context = { cwd: process.cwd(), sessionManager: { getSessionId: () => "pi-roundtrip" } };
  // The host's recording snapshot, not a later disk setting, travels to exec.
  execFileSync(process.env.OUTRIM_BINARY, ["gain", "record", "off"]);
  try {
    const path = process.env.OUTRIM_FIXTURE.replaceAll("\\", "/");
    const event = {
      toolName: "bash",
      toolCallId: "wrapped",
      input: { command: `'${path}' test --report` },
    };
    await handlers.get("tool_call")(event, context);
    assert.ok(event.input.command.includes("exec --context"));
    const output = spawnSync("bash", ["-c", event.input.command], { encoding: "utf8" });
    assert.equal(output.status, 19, output.stderr);
    for (const [stream, text] of Object.entries({ stdout: output.stdout, stderr: output.stderr })) {
      const path = text.split("The full output is available at ")[1]?.trim();
      assert.ok(path, text);
      const original = readFileSync(path, "utf8");
      assert.ok(original.includes("running 100 tests"));
      assert.ok(original.includes(`test ${stream}_case_99 ... ok`));
      assert.equal(original.includes(stream === "stdout" ? "stderr_case" : "stdout_case"), false);
    }
    assert.equal(readFileSync("executed", "utf8"), "once");
    assert.equal(
      await handlers.get("tool_result")(
        { ...event, content: [{ type: "text", text: output.stdout + output.stderr }] },
        context,
      ),
      undefined,
    );
    for (const command of [
      "cargo test | sort",
      "outrim exec -- cargo test",
      "outrim exec --no-compress -- cargo test",
    ]) {
      const input = { command };
      await handlers.get("tool_call")({ ...event, input }, context);
      assert.equal(input.command, command);
      await handlers.get("tool_result")(
        { ...event, input, content: [{ type: "text", text: "must not account" }] },
        context,
      );
    }
    const original = output.stdout.split("The full output is available at ")[1].trim();
    await handlers.get("tool_result")(
      {
        toolName: "read",
        toolCallId: "recovery",
        input: { path: original },
        content: [{ type: "text", text: readFileSync(original, "utf8") }],
      },
      context,
    );
    const json = JSON.stringify(
      Array.from({ length: 100 }, () => ({ name: "工具", value: 123 })),
      null,
      2,
    );
    const native = await handlers.get("tool_result")(
      {
        toolName: "custom",
        toolCallId: "native",
        input: {},
        content: [{ type: "text", text: json }],
      },
      context,
    );
    assert.ok(native.content[0].text.length < json.length);
    const source = resolve("dedup source.rs");
    const body = "fn source_content() {}\n".repeat(200);
    writeFileSync(source, body);
    for (const toolName of ["read", "bash"]) {
      for (let index = 0; index < 2; index++) {
        const event = {
          toolName,
          toolCallId: `${toolName}-${index}`,
          input:
            toolName === "read"
              ? { path: source }
              : { command: `cat '${source.replaceAll("\\", "/")}'` },
        };
        await handlers.get("tool_call")(event, context);
        const text =
          toolName === "read"
            ? body
            : spawnSync("bash", ["-c", event.input.command], { encoding: "utf8" }).stdout;
        const result = await handlers.get("tool_result")(
          { ...event, content: [{ type: "text", text }] },
          context,
        );
        assert.equal(
          index === 0
            ? (result?.content[0].text ?? text)
            : result?.content[0].text.includes("unchanged, so omitted"),
          index === 0 ? body : true,
        );
      }
    }
  } finally {
    handlers.get("session_shutdown")();
  }
} else {
  const executable = resolve(process.argv[2]);
  const home = mkdtempSync(join(tmpdir(), "outrim-pi-roundtrip-"));
  const fixture = join(home, process.platform === "win32" ? "cargo.exe" : "cargo");
  const env = {
    ...process.env,
    HOME: home,
    USERPROFILE: home,
    PI_CODING_AGENT_DIR: join(home, "pi"),
    OUTRIM_BINARY: executable,
    OUTRIM_FIXTURE: fixture,
  };
  try {
    execFileSync("rustc", [
      "--edition=2024",
      join(root, "tests/fixtures/exec-child.rs"),
      "-o",
      fixture,
    ]);
    execFileSync(executable, ["install", "--agent", "pi"], { env });
    execFileSync(executable, ["gain", "record", "on"], { env });
    execFileSync(process.execPath, [import.meta.filename, "--host"], {
      env,
      cwd: home,
      stdio: "inherit",
      timeout: 30_000,
    });
    const stats = JSON.parse(readFileSync(join(home, ".outrim/gain.json"), "utf8"));
    assert.equal(
      Object.values(stats.outcomes).reduce((total, count) => total + count, 0),
      8,
      "two streams + recovery + custom tool + four delivered file reads",
    );
    assert.equal(stats.rules.read_dedup.count, 2);
    assert.ok(stats.rules.read_dedup.after_chars < stats.rules.read_dedup.before_chars);
    const events = readdirSync(join(home, ".outrim/records")).flatMap((instance) =>
      readFileSync(join(home, ".outrim/records", instance, "events.jsonl"), "utf8")
        .trim()
        .split("\n")
        .map(JSON.parse),
    );
    const streams = events.filter((event) => event.tool_call_id === "wrapped" && event.execution);
    assert.deepEqual(streams.map((event) => event.execution.stream).sort(), ["stderr", "stdout"]);
    for (const event of streams) {
      assert.equal(event.host, "pi");
      assert.equal(event.session, "pi-roundtrip");
      assert.equal(event.execution.unit_id, "wrapped:0");
      assert.equal(event.execution.recording, true);
    }
    assert.equal(
      events.filter((event) => event.outcome === "recovery_skipped" && !event.metadata_only).length,
      1,
    );
    const recovery = events.filter((event) => event.diagnostics?.recovery);
    assert.equal(recovery.length, 1);
    assert.equal(stats.recovery_chars, recovery[0].diagnostics.recovery.characters);
    assert.ok(stats.recovery_chars > 0);
    const stdout = streams.find((event) => event.execution.stream === "stdout");
    assert.deepEqual(recovery[0].original_references, [
      { original_path: stdout.original_path, record_id: stdout.event_id },
    ]);
    const metadata = events.filter((event) => event.metadata_only);
    assert.ok(metadata.length > 0);
    assert.ok(
      metadata.every((event) => Object.values(event.content).every((path) => path === null)),
    );
    console.log(
      "pi: installed bundle → rewrite → native shell → per-stream originals/gain/records passed",
    );
  } finally {
    rmSync(home, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
}
