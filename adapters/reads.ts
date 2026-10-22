import { realpathSync, statSync } from "node:fs";
import { isAbsolute, resolve } from "node:path";

export interface ReadSelection {
  path: string;
  key: string;
  label: string;
}

// Selection facts come from the host, never from read-looking output of another tool.
export function nativeReadSelection(
  host: "pi" | "opencode",
  input: Record<string, unknown>,
  text: string,
  cwd: string | undefined,
): ReadSelection | undefined {
  const path = host === "opencode" ? (input.filePath ?? input.path) : input.path;
  if (typeof path !== "string" || (!isAbsolute(path) && !cwd)) return undefined;
  const start = input.offset ?? 1;
  const limit = input.limit;
  if (
    !Number.isSafeInteger(start) ||
    (start as number) < 1 ||
    (limit !== undefined && (!Number.isSafeInteger(limit) || (limit as number) < 1))
  )
    return undefined;
  let label: string;
  if (host === "opencode") {
    const range = /^Read file [^\r\n]+, lines (\d+)-(\d+)\r?\n/.exec(text);
    if (!range || Number(range[1]) !== start) return undefined;
    label = `${range[1]}–${range[2]}`;
  } else {
    // These footers are host truncation, not file contents. Keep them intact.
    if (/\[(?:Showing lines |\d+ more lines in file\.)/.test(text)) return undefined;
    const count = text.split("\n").length - (text.endsWith("\n") ? 1 : 0);
    if (!count) return undefined;
    label = `${start}–${(start as number) + count - 1}`;
  }
  try {
    const absolute = realpathSync(resolve(cwd ?? "", path));
    if (!statSync(absolute).isFile()) return undefined;
    return { path: absolute, key: JSON.stringify([host, start, limit ?? null]), label };
  } catch {
    return undefined;
  }
}
