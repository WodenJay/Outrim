import { expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const shellPath = (path) =>
  path.replaceAll("\\", "/").replace(/^([A-Za-z]):/, (_, drive) => `/${drive.toLowerCase()}`);
const installer = shellPath(resolve("scripts/install.sh"));

function fixture(os = "Linux", arch = "x86_64", fail = false) {
  const root = mkdtempSync(join(tmpdir(), "outrim-distribution-"));
  const bin = join(root, "commands");
  mkdirSync(bin);
  const payload = "local release binary\n";
  writeFileSync(join(root, "binary"), payload);
  writeFileSync(
    join(bin, "uname"),
    `#!/bin/sh\ncase "$1" in -s) echo '${os}';; -m) echo '${arch}';; esac\n`,
    { mode: 0o755 },
  );
  writeFileSync(
    join(bin, "curl"),
    `#!/bin/sh
out=''
while [ "$#" -gt 0 ]; do
  case "$1" in -o) out=$2; shift 2;; *) url=$1; shift;; esac
done
printf '%s\\n' "$url" >> "$HOME/requests"
${
  fail
    ? "exit 22"
    : `case "$url" in
  */SHA256SUMS) cp "$HOME/checksums" "$out";;
  *) cp "$HOME/binary" "$out";;
esac`
}
`,
    { mode: 0o755 },
  );
  return {
    root,
    payload,
    run(args = []) {
      return spawnSync(
        "bash",
        ["-c", 'export PATH="$TEST_BIN:$PATH"; exec sh "$INSTALLER" "$@"', "test", ...args],
        {
          encoding: "utf8",
          env: {
            ...process.env,
            HOME: shellPath(root),
            TEST_BIN: shellPath(bin),
            INSTALLER: installer,
            SHELL: "/bin/bash",
          },
        },
      );
    },
    checksum(name, content = payload) {
      writeFileSync(
        join(root, "checksums"),
        `${createHash("sha256").update(content).digest("hex")}  ${name}\n`,
      );
    },
    clean() {
      rmSync(root, { recursive: true, force: true });
    },
  };
}

for (const [os, arch, asset] of [
  ["Linux", "x86_64", "outrim-x86_64-unknown-linux-gnu"],
  ["Linux", "aarch64", "outrim-aarch64-unknown-linux-gnu"],
  ["Darwin", "arm64", "outrim-aarch64-apple-darwin"],
]) {
  test(`shell installer selects ${asset} and configures PATH once`, () => {
    const f = fixture(os, arch);
    try {
      f.checksum(asset);
      const args = ["--version", "v0.1.0"];
      let result = f.run(args);
      expect(result.stderr).toBe("");
      expect(result.status).toBe(0);
      expect(readFileSync(join(f.root, ".outrim/bin/outrim"), "utf8")).toBe(f.payload);
      expect(readFileSync(join(f.root, "requests"), "utf8")).toContain(
        `https://github.com/WodenJay/Outrim/releases/download/v0.1.0/${asset}`,
      );
      const profile = readFileSync(join(f.root, ".profile"), "utf8");
      result = f.run(args);
      expect(result.status).toBe(0);
      expect(readFileSync(join(f.root, ".profile"), "utf8")).toBe(profile);
      expect(readFileSync(join(f.root, ".bashrc"), "utf8")).toContain(".outrim/bin");
    } finally {
      f.clean();
    }
  });
}

test("shell installer accepts a trusted local artifact without download or PATH changes", () => {
  const f = fixture();
  try {
    const result = f.run(["--from-file", shellPath(join(f.root, "binary")), "--no-path"]);
    expect(result.status).toBe(0);
    expect(readFileSync(join(f.root, ".outrim/bin/outrim"), "utf8")).toBe(f.payload);
    expect(existsSync(join(f.root, "requests"))).toBe(false);
    expect(existsSync(join(f.root, ".profile"))).toBe(false);
    expect(readdirSync(join(f.root, ".outrim"))).toEqual(["bin"]);
  } finally {
    f.clean();
  }
});

test("unsupported macOS Intel fails before download", () => {
  const f = fixture("Darwin", "x86_64");
  try {
    const result = f.run();
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain("Unsupported platform");
    expect(existsSync(join(f.root, "requests"))).toBe(false);
  } finally {
    f.clean();
  }
});

for (const failDownload of [false, true]) {
  test(`${failDownload ? "download failure" : "checksum mismatch"} preserves an installed binary`, () => {
    const f = fixture("Linux", "x86_64", failDownload);
    try {
      mkdirSync(join(f.root, ".outrim/bin"), { recursive: true });
      writeFileSync(join(f.root, ".outrim/bin/outrim"), "previous binary");
      f.checksum("outrim-x86_64-unknown-linux-gnu", "wrong payload");
      const result = f.run();
      expect(result.status).not.toBe(0);
      if (!failDownload) expect(result.stderr).toContain("Checksum mismatch");
      expect(readFileSync(join(f.root, ".outrim/bin/outrim"), "utf8")).toBe("previous binary");
    } finally {
      f.clean();
    }
  });
}

const windowsTest = process.platform === "win32" ? test : test.skip;
for (const failure of ["none", "checksum", "download"]) {
  windowsTest(`PowerShell download: ${failure}`, () => {
    const root = mkdtempSync(join(tmpdir(), "outrim-ps-download-"));
    try {
      const payload = "Windows download fixture";
      writeFileSync(join(root, "binary"), payload);
      mkdirSync(join(root, ".outrim/bin"), { recursive: true });
      writeFileSync(join(root, ".outrim/bin/outrim.exe"), "previous binary");
      const hash = createHash("sha256")
        .update(failure === "checksum" ? "wrong binary" : payload)
        .digest("hex");
      writeFileSync(join(root, "SHA256SUMS"), `${hash}  outrim-x86_64-pc-windows-msvc.exe\n`);
      const wrapper = join(root, "download-test.ps1");
      writeFileSync(
        wrapper,
        `
$ErrorActionPreference = 'Stop'
function Invoke-WebRequest {
    param($Uri, $OutFile, [switch]$UseBasicParsing)
    Add-Content -LiteralPath (Join-Path $env:USERPROFILE 'requests') -Value $Uri
    if ($env:TEST_FAILURE -eq 'download') { throw 'Simulated download failure' }
    $Source = if ($Uri.EndsWith('/SHA256SUMS')) { 'SHA256SUMS' } else { 'binary' }
    Copy-Item -LiteralPath (Join-Path $env:USERPROFILE $Source) -Destination $OutFile
}
& $env:TEST_INSTALLER -Version v0.1.0 -NoPath
`,
      );
      const result = spawnSync("pwsh", ["-NoProfile", "-File", wrapper], {
        encoding: "utf8",
        env: {
          ...process.env,
          USERPROFILE: root,
          TEST_FAILURE: failure,
          TEST_INSTALLER: resolve("scripts/install.ps1"),
        },
      });
      expect(result.status).toBe(failure === "none" ? 0 : 1);
      expect(readFileSync(join(root, "requests"), "utf8")).toContain(
        "https://github.com/WodenJay/Outrim/releases/download/v0.1.0/outrim-x86_64-pc-windows-msvc.exe",
      );
      expect(readFileSync(join(root, ".outrim/bin/outrim.exe"), "utf8")).toBe(
        failure === "none" ? payload : "previous binary",
      );
      if (failure === "checksum") expect(result.stderr).toContain("Checksum mismatch");
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
}
windowsTest(
  "PowerShell installs a local artifact without changing user PATH or agent config",
  () => {
    const root = mkdtempSync(join(tmpdir(), "outrim-powershell-"));
    try {
      const source = join(root, "source.exe");
      writeFileSync(source, "local Windows release binary");
      const result = spawnSync(
        "pwsh",
        ["-NoProfile", "-File", "scripts/install.ps1", "-FromFile", source, "-NoPath"],
        {
          encoding: "utf8",
          env: { ...process.env, USERPROFILE: root },
        },
      );
      expect(result.stderr).toBe("");
      expect(result.status).toBe(0);
      expect(readFileSync(join(root, ".outrim/bin/outrim.exe"), "utf8")).toBe(
        "local Windows release binary",
      );
      expect(readdirSync(join(root, ".outrim"))).toEqual(["bin"]);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  },
);
