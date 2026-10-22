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
    join(root, "release-url"),
    "https://github.com/WodenJay/Outrim/releases/tag/v0.1.0\n",
  );
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
  case "$1" in -o) out=$2; shift 2;; -w) shift 2;; *) url=$1; shift;; esac
done
printf '%s\\n' "$url" >> "$HOME/requests"
${
  fail
    ? "exit 22"
    : `case "$url" in
  */latest)
    IFS= read -r release < "$HOME/release-url"
    printf '%s' "$release"
    printf '%s\\n' 'https://github.com/WodenJay/Outrim/releases/tag/v9.9.9' > "$HOME/release-url";;
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
      expect(readFileSync(join(f.root, "requests"), "utf8")).not.toContain("/latest");
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
      const result = f.run(["--version", "v0.1.0"]);
      expect(result.status).not.toBe(0);
      if (!failDownload) expect(result.stderr).toContain("Checksum mismatch");
      expect(readFileSync(join(f.root, ".outrim/bin/outrim"), "utf8")).toBe("previous binary");
      expect(readdirSync(join(f.root, ".outrim/bin"))).toEqual(["outrim"]);
    } finally {
      f.clean();
    }
  });
}

test("shell latest pins both downloads before the release changes", () => {
  const f = fixture();
  try {
    f.checksum("outrim-x86_64-unknown-linux-gnu");
    const result = f.run(["--no-path"]);
    expect(result.stderr).toBe("");
    expect(result.status).toBe(0);
    expect(readFileSync(join(f.root, "requests"), "utf8").trim().split("\n")).toEqual([
      "https://github.com/WodenJay/Outrim/releases/latest",
      "https://github.com/WodenJay/Outrim/releases/download/v0.1.0/outrim-x86_64-unknown-linux-gnu",
      "https://github.com/WodenJay/Outrim/releases/download/v0.1.0/SHA256SUMS",
    ]);
    expect(readFileSync(join(f.root, ".outrim/bin/outrim"), "utf8")).toBe(f.payload);
    expect(readdirSync(join(f.root, ".outrim/bin"))).toEqual(["outrim"]);
  } finally {
    f.clean();
  }
});

for (const release of [
  null,
  "https://github.com/WodenJay/Outrim/releases/latest",
  "https://github.com/WodenJay/Outrim/releases/tag/",
  "https://github.com/WodenJay/Outrim/releases/tag/latest",
  "https://github.com/WodenJay/Outrim/releases/tag/v1/invalid",
  "https://example.com/releases/tag/v1",
]) {
  test(`shell rejects unresolved latest: ${release}`, () => {
    const f = fixture("Linux", "x86_64", release === null);
    try {
      if (release !== null) writeFileSync(join(f.root, "release-url"), `${release}\n`);
      mkdirSync(join(f.root, ".outrim/bin"), { recursive: true });
      writeFileSync(join(f.root, ".outrim/bin/outrim"), "previous binary");
      const result = f.run(["--no-path"]);
      expect(result.status).not.toBe(0);
      expect(readFileSync(join(f.root, "requests"), "utf8").trim()).toBe(
        "https://github.com/WodenJay/Outrim/releases/latest",
      );
      expect(readFileSync(join(f.root, ".outrim/bin/outrim"), "utf8")).toBe("previous binary");
      expect(readdirSync(join(f.root, ".outrim/bin"))).toEqual(["outrim"]);
    } finally {
      f.clean();
    }
  });
}

const windowsTest = process.platform === "win32" ? test : test.skip;
for (const runtime of ["pwsh", "powershell"]) {
  for (const failure of [
    "none",
    "checksum",
    "download",
    "latest",
    "unresolved",
    "invalid",
    "resolve",
  ]) {
    windowsTest(`${runtime} download: ${failure}`, () => {
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
    param($Uri, $OutFile, $Method, [switch]$UseBasicParsing)
    Add-Content -LiteralPath (Join-Path $env:USERPROFILE 'requests') -Value $Uri
    if ($Uri.EndsWith('/latest')) {
        if ($env:TEST_FAILURE -eq 'resolve') { throw 'Simulated resolution failure' }
        if ($Method -ne 'Head') { throw 'Expected HEAD request' }
        $FinalUri = switch ($env:TEST_FAILURE) {
            'unresolved' { $Uri }
            'invalid' { 'https://github.com/WodenJay/Outrim/releases/tag/v1/invalid' }
            default { 'https://github.com/WodenJay/Outrim/releases/tag/v0.1.0' }
        }
        if ($script:Resolved) { $FinalUri = 'https://github.com/WodenJay/Outrim/releases/tag/v9.9.9' }
        $script:Resolved = $true
        $Response = if ($PSVersionTable.PSVersion.Major -ge 6) {
            @{ RequestMessage = @{ RequestUri = [uri]$FinalUri } }
        } else {
            @{ ResponseUri = [uri]$FinalUri }
        }
        return @{ BaseResponse = $Response }
    }
    if ($env:TEST_FAILURE -eq 'download') { throw 'Simulated download failure' }
    $Source = if ($Uri.EndsWith('/SHA256SUMS')) { 'SHA256SUMS' } else { 'binary' }
    Copy-Item -LiteralPath (Join-Path $env:USERPROFILE $Source) -Destination $OutFile
}
if ($env:TEST_FAILURE -in @('none', 'checksum', 'download')) {
    & $env:TEST_INSTALLER -Version v0.1.0 -NoPath
} else {
    & $env:TEST_INSTALLER -NoPath
}
`,
        );
        const result = spawnSync(runtime, ["-NoProfile", "-File", wrapper], {
          encoding: "utf8",
          env: {
            ...process.env,
            USERPROFILE: root,
            // Do not let inherited PowerShell 7 modules shadow Windows PowerShell modules.
            PSModulePath:
              runtime === "powershell"
                ? `${process.env.SystemRoot}\\System32\\WindowsPowerShell\\v1.0\\Modules`
                : process.env.PSModulePath,
            TEST_FAILURE: failure,
            TEST_INSTALLER: resolve("scripts/install.ps1"),
          },
        });
        const success = failure === "none" || failure === "latest";
        expect(result.status).toBe(success ? 0 : 1);
        const requests = readFileSync(join(root, "requests"), "utf8").trim().split(/\r?\n/);
        const base = "https://github.com/WodenJay/Outrim/releases";
        if (["unresolved", "invalid", "resolve"].includes(failure)) {
          expect(requests).toEqual([`${base}/latest`]);
        } else if (failure === "latest") {
          expect(requests).toEqual([
            `${base}/latest`,
            `${base}/download/v0.1.0/outrim-x86_64-pc-windows-msvc.exe`,
            `${base}/download/v0.1.0/SHA256SUMS`,
          ]);
        } else {
          expect(requests[0]).toBe(`${base}/download/v0.1.0/outrim-x86_64-pc-windows-msvc.exe`);
          expect(requests).not.toContain(`${base}/latest`);
        }
        expect(readFileSync(join(root, ".outrim/bin/outrim.exe"), "utf8")).toBe(
          success ? payload : "previous binary",
        );
        expect(readdirSync(join(root, ".outrim/bin"))).toEqual(["outrim.exe"]);
        if (failure === "checksum") expect(result.stderr).toContain("Checksum mismatch");
      } finally {
        rmSync(root, { recursive: true, force: true });
      }
    });
  }
  windowsTest(
    `${runtime} installs a local artifact without changing user PATH or agent config`,
    () => {
      const root = mkdtempSync(join(tmpdir(), "outrim-powershell-"));
      try {
        const source = join(root, "source.exe");
        writeFileSync(source, "local Windows release binary");
        const result = spawnSync(
          runtime,
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
}
