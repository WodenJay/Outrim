/**
 * Rebuilds the two committed host bundles under `assets/`.
 *
 * Outrim embeds these files with `include_str!`, so they must be reproducible:
 * `bun run build` followed by `git diff --exit-code assets` has to stay clean.
 */

import { createHash } from "node:crypto";
import { join } from "node:path";

const entries = [
  { entrypoint: "adapters/pi.ts", outfile: "assets/outrim-pi.js" },
  { entrypoint: "adapters/opencode-plugin.ts", outfile: "assets/outrim-opencode.js" },
];

const root = join(import.meta.dir, "..");
let failed = false;
for (const { entrypoint, outfile } of entries) {
  const result = await Bun.build({
    entrypoints: [join(root, entrypoint)],
    target: "bun",
    format: "esm",
  });
  const output = result.outputs[0];
  if (!result.success || result.outputs.length !== 1 || !output) {
    failed = true;
    for (const log of result.logs) console.error(log);
    continue;
  }
  const bundled = await output.text();
  const identity = createHash("sha256").update(bundled).digest("hex");
  await Bun.write(join(root, outfile), bundled.replaceAll("__OUTRIM_ADAPTER_BUILD__", identity));
  console.log(`${outfile} <- ${entrypoint}`);
}

process.exit(failed ? 1 : 0);
