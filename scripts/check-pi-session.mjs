// Offline E2E: scripted provider -> Pi agent loop -> installed extension -> real Bash.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

// Point at the installed @earendil-works/pi-coding-agent package directory.
assert.ok(
  process.env.PI_SDK_DIR,
  "Set PI_SDK_DIR to the installed Pi SDK package directory (tested with Pi 1.3.0 and 1.4.0)",
);
const sdkDir = resolve(process.env.PI_SDK_DIR);
assert.ok(existsSync(join(sdkDir, "dist/index.js")), `Pi SDK not found at ${sdkDir}`);
if (process.argv[2] !== "--host") {
  const sourceBinary = resolve(
    process.argv[2] || `target/debug/outrim${process.platform === "win32" ? ".exe" : ""}`,
  );
  const home = mkdtempSync(join(tmpdir(), "outrim-pi-session-"));
  const binary = join(home, process.platform === "win32" ? "outrim.exe" : "outrim");
  copyFileSync(sourceBinary, binary);
  const env = {
    ...process.env,
    HOME: home,
    USERPROFILE: home,
    PI_CODING_AGENT_DIR: join(home, "pi"),
    OUTRIM_BINARY: binary,
    PI_SDK_DIR: sdkDir,
  };
  try {
    // Make the installed extension's SDK import resolve to the exact host SDK.
    symlinkSync(
      resolve(sdkDir, "../.."),
      join(home, "node_modules"),
      process.platform === "win32" ? "junction" : "dir",
    );
    writeFileSync(
      join(home, "fixture.rs"),
      String.raw`use std::{fs::OpenOptions,io::Write,process::ExitCode};
fn main()->ExitCode {
 let args:Vec<_>=std::env::args().collect();
 OpenOptions::new().create(true).append(true).open("executed").unwrap().write_all(format!("{}\n",args[1..].join(" ")).as_bytes()).unwrap();
 print!("{}    Finished stdout fixture\n",(0..100).map(|n|format!("   Compiling stdout-{n} v1.0.0\n")).collect::<String>());
 eprint!("{}warning: stderr fixture\n",(0..100).map(|n|format!("    Checking stderr-{n} v1.0.0\n")).collect::<String>());
 if args.iter().any(|v|v=="--fail") {ExitCode::from(19)}else{ExitCode::SUCCESS}
}`,
    );
    execFileSync(
      "rustc",
      [
        "--edition=2024",
        join(home, "fixture.rs"),
        "-o",
        join(home, process.platform === "win32" ? "cargo.exe" : "cargo"),
      ],
      { stdio: "inherit" },
    );
    execFileSync(binary, ["install", "--agent", "pi"], { env, stdio: "inherit" });
    execFileSync(binary, ["gain", "record", "on"], { env });
    execFileSync(process.execPath, [import.meta.filename, "--host"], {
      cwd: home,
      env,
      stdio: "inherit",
      timeout: 90000,
    });
  } finally {
    if (process.env.OUTRIM_KEEP_TEST_HOME) console.log(`test home: ${home}`);
    else rmSync(home, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
} else {
  const sdk = await import(pathToFileURL(join(sdkDir, "dist/index.js")).href);
  const ai = await import(pathToFileURL(join(dirname(sdkDir), "pi-ai/dist/index.js")).href);
  const cwd = process.cwd();
  const quote = (text) => `'${text.replaceAll("\\", "/").replaceAll("'", "'\\''")}'`;
  const cargo = quote(join(cwd, process.platform === "win32" ? "cargo.exe" : "cargo"));
  const commands = [
    `${cargo} build ; ${cargo} build`,
    `${cargo} build --fail && ${cargo} build --never`,
    `${cargo} build | sort`,
  ];
  const modelDefinition = {
    id: "scripted",
    name: "Offline scripted",
    api: "outrim-offline",
    reasoning: false,
    input: ["text"],
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
    contextWindow: 100000,
    maxTokens: 4096,
  };
  let request = 0;
  let recoveryPaths = [];
  const results = [];
  const settingsManager = sdk.SettingsManager.inMemory({
    compaction: { enabled: false },
    retry: { enabled: false },
    shellPath: process.env.PI_TEST_BASH || sdk.getShellConfig().shell,
  });
  const loader = new sdk.DefaultResourceLoader({
    cwd,
    agentDir: process.env.PI_CODING_AGENT_DIR,
    settingsManager,
    noSkills: true,
    noPromptTemplates: true,
    noThemes: true,
    extensionFactories: [
      (pi) => {
        pi.registerProvider("outrim-offline", {
          api: "outrim-offline",
          apiKey: "offline",
          baseUrl: "http://invalid.local",
          models: [modelDefinition],
          streamSimple(model) {
            const stream = ai.createAssistantMessageEventStream();
            queueMicrotask(() => {
              let content;
              if (request < commands.length)
                content = [
                  {
                    type: "toolCall",
                    id: `shell-${request}`,
                    name: "bash",
                    arguments: { command: commands[request], timeout: 20 },
                  },
                ];
              else if (request === commands.length) {
                recoveryPaths = results
                  .filter((r) => r.toolName === "bash")
                  .slice(0, 2)
                  .flatMap((r) =>
                    [
                      ...r.content
                        .map((p) => p.text || "")
                        .join("\n")
                        .matchAll(/The full output is available at ([^\r\n]+?\.txt)/g),
                    ].map((m) => m[1].trim()),
                  );
                assert.equal(
                  recoveryPaths.length,
                  6,
                  `two safe units + one failed unit, each with two compressed streams: ${JSON.stringify(results)}`,
                );
                content = recoveryPaths.map((path, i) => ({
                  type: "toolCall",
                  id: `recover-${i}`,
                  name: "read",
                  arguments: { path },
                }));
              } else content = [{ type: "text", text: "offline session complete" }];
              request++;
              const message = {
                role: "assistant",
                content,
                api: model.api,
                provider: model.provider,
                model: model.id,
                usage: {
                  input: 0,
                  output: 0,
                  cacheRead: 0,
                  cacheWrite: 0,
                  totalTokens: 0,
                  cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
                },
                stopReason: content[0].type === "toolCall" ? "toolUse" : "stop",
                timestamp: Date.now(),
              };
              stream.push({ type: "done", reason: message.stopReason, message });
              stream.end();
            });
            return stream;
          },
        });
      },
    ],
  });
  await loader.reload();
  assert.deepEqual(loader.getExtensions().errors, []);
  const modelRuntime = await sdk.ModelRuntime.create({
    authPath: join(cwd, "auth.json"),
    modelsPath: join(cwd, "models.json"),
  });
  const { session } = await sdk.createAgentSession({
    cwd,
    agentDir: process.env.PI_CODING_AGENT_DIR,
    resourceLoader: loader,
    modelRuntime,
    settingsManager,
    tools: ["bash", "read"],
    sessionManager: sdk.SessionManager.inMemory(cwd),
    model: { ...modelDefinition, provider: "outrim-offline", baseUrl: "http://invalid.local" },
    thinkingLevel: "off",
  });
  session.subscribe((event) => {
    if (event.type === "message_end" && event.message.role === "toolResult")
      results.push(event.message);
  });
  try {
    await session.bindExtensions({});
    await session.prompt("Execute the offline scripted verification.");
    assert.equal(request, 5);
    assert.equal(results.length, 9);
    assert.equal(results[0].isError, false);
    assert.equal(results[1].isError, true);
    assert.equal(results[2].isError, false);
    assert.ok(
      !results[2].content
        .map((p) => p.text || "")
        .join("")
        .includes("The full output is available at"),
      "pipeline fallback remains native",
    );
    assert.deepEqual(readFileSync("executed", "utf8").trim().split("\n"), [
      "build",
      "build",
      "build --fail",
      "build",
    ]);
    assert.equal(new Set(recoveryPaths).size, 6);
    for (let i = 0; i < recoveryPaths.length; i++) {
      const original = readFileSync(recoveryPaths[i], "utf8");
      const stderr = i % 2 === 1;
      const expected =
        Array.from({ length: 100 }, (_, n) =>
          stderr ? `    Checking stderr-${n} v1.0.0\n` : `   Compiling stdout-${n} v1.0.0\n`,
        ).join("") + (stderr ? "warning: stderr fixture\n" : "    Finished stdout fixture\n");
      assert.equal(original, expected);
      assert.equal(results[i + 3].content.map((p) => p.text || "").join(""), original);
    }
  } finally {
    await session.extensionRunner.emit({ type: "session_shutdown" });
    session.dispose();
  }
  // Bridge observations and shutdown are asynchronous; wait for persisted delivery.
  const deadline = Date.now() + 5000;
  while (Date.now() < deadline) {
    const stats = JSON.parse(readFileSync(join(cwd, ".outrim/gain.json"), "utf8"));
    if (Object.values(stats.outcomes).reduce((total, count) => total + count, 0) === 12) break;
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  const stats = JSON.parse(readFileSync(join(cwd, ".outrim/gain.json"), "utf8"));
  const events = readdirSync(join(cwd, ".outrim/records")).flatMap((instance) =>
    readFileSync(join(cwd, ".outrim/records", instance, "events.jsonl"), "utf8")
      .trim()
      .split("\n")
      .filter(Boolean)
      .map(JSON.parse),
  );
  // Wrapping/observation diagnostics are not delivered-output accounting samples.
  const samples = events.filter((event) => !event.metadata_only);
  const shell = samples.filter((event) => event.execution);
  assert.ok(
    events.some((event) => event.metadata_only),
    "adapter diagnostics are recorded",
  );
  assert.equal(shell.length, 6, JSON.stringify(events));
  assert.equal(
    samples.length,
    12,
    "6 shell streams + 6 recovery reads; no shell result double counting",
  );
  assert.equal(
    Object.values(stats.outcomes).reduce((total, count) => total + count, 0),
    12,
  );
  assert.equal(stats.outcomes.compressed, 6);
  assert.equal(stats.outcomes.recovery_skipped, 6);
  assert.equal(stats.rules.log_compaction.count, 6);
  assert.equal(samples.filter((e) => e.outcome === "recovery_skipped").length, 6);
  assert.equal(
    new Set(shell.map((e) => `${e.tool_call_id}:${e.execution.unit_id}:${e.execution.stream}`))
      .size,
    6,
  );
  for (const id of ["shell-0", "shell-1"]) {
    const units = Map.groupBy(
      shell.filter((e) => e.tool_call_id === id),
      (e) => e.execution.unit_id,
    );
    assert.equal(units.size, id === "shell-0" ? 2 : 1);
    for (const rows of units.values())
      assert.deepEqual(rows.map((e) => e.execution.stream).sort(), ["stderr", "stdout"]);
  }
  console.log(
    "pi-session: real SDK provider → agent → installed extension → Bash: 3 calls, 3 wrapped units, 6 compressed streams/recovered originals, && short-circuit, pipeline fallback, 12 accounting outputs; passed",
  );
}
