import { realpathSync, statSync } from "node:fs";
import { isAbsolute, resolve } from "node:path";

interface ReadBody {
  start: number;
  prefix?: string;
  lines: { text: string; rendered: string }[];
  suffix: string;
  final_line?: { text: string; rendered: string };
}

export interface ReadSelection {
  path: string;
  key: string;
  label: string;
  body?: ReadBody;
}

function piBody(text: string, start: number, facts: Record<string, unknown>): ReadBody | undefined {
  if (facts.firstLineExceedsLimit === true) return undefined;
  const notice =
    /\n\n?\[(?:Showing lines (\d+)-(\d+) of \d+(?: \([^\r\n]+ limit\))?\. Use offset=(\d+) to continue\.|\d+ more lines in file\. Use offset=(\d+) to continue\.)\]$/.exec(
      text,
    );
  if (facts.truncated === true && !notice) return undefined;
  if (/\[(?:Showing lines |\d+ more lines in file\.)/.test(text) && !notice) return undefined;
  const source = notice ? text.slice(0, notice.index) : text;
  const chunks = source.split("\n");
  if (notice) {
    const end = start + chunks.length - 1;
    if (notice[1] && (Number(notice[1]) !== start || Number(notice[2]) !== end)) return undefined;
    if (Number(notice[3] ?? notice[4]) !== end + 1) return undefined;
  }
  const lines = chunks.slice(0, -1).map((line) => ({ text: `${line}\n`, rendered: `${line}\n` }));
  const final_line = chunks.at(-1);
  if (!lines.length && !final_line) return undefined;
  return {
    start,
    lines,
    ...(final_line ? { final_line: { text: final_line, rendered: final_line } } : {}),
    suffix: text.slice(lines.reduce((size, line) => size + line.rendered.length, 0)),
  };
}

function openCodeBody(
  text: string,
  start: number,
  end: number,
  prefix: string,
  facts: Record<string, unknown>,
): ReadBody | undefined {
  if (facts.linesTruncated === true) return undefined;
  const rest = text.slice(prefix.length);
  const notice = /\n\[Output truncated\. Continue reading with offset: (\d+)\]$/.exec(rest);
  if (facts.truncated === true && !notice) return undefined;
  if (notice && Number(notice[1]) !== end + 1) return undefined;
  const source = notice ? rest.slice(0, notice.index) : rest;
  const chunks = source.split("\n");
  if (chunks.length !== end - start + 1) return undefined;
  const lines: ReadBody["lines"] = [];
  for (let index = 0; index < chunks.length; index++) {
    const number = `${start + index}: `;
    const chunk = chunks[index];
    if (chunk === undefined || !chunk.startsWith(number)) return undefined;
    // Both hosts discard terminal newline information. The final displayed
    // source line (even when paginated) is deliberately not range evidence.
    if (index < chunks.length - 1)
      lines.push({ text: `${chunk.slice(number.length)}\n`, rendered: `${chunk}\n` });
  }
  const final_line = chunks.at(-1);
  if (!final_line) return undefined;
  return {
    start,
    prefix,
    lines,
    final_line: { text: final_line.slice(`${end}: `.length), rendered: final_line },
    suffix: rest.slice(lines.reduce((size, line) => size + line.rendered.length, 0)),
  };
}

// Selection facts come from the host, never from read-looking output of another tool.
export function nativeReadSelection(
  host: "pi" | "opencode",
  input: Record<string, unknown>,
  text: string,
  cwd: string | undefined,
  facts: Record<string, unknown> = {},
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
  let body: ReadBody | undefined;
  if (host === "opencode") {
    const range = /^Read file [^\r\n]+, lines (\d+)-(\d+)\n/.exec(text);
    if (!range || Number(range[1]) !== start || Number(range[2]) < (start as number))
      return undefined;
    label = `${range[1]}–${range[2]}`;
    body = openCodeBody(text, start as number, Number(range[2]), range[0], facts);
  } else {
    if (!text) return undefined;
    body = piBody(text, start as number, facts);
    const count = text.split("\n").length - (text.endsWith("\n") ? 1 : 0);
    const notice =
      /\[(?:Showing lines (\d+)-(\d+) of |\d+ more lines in file\. Use offset=(\d+) to continue\.)/.exec(
        text,
      );
    const end = notice ? Number(notice[2] ?? Number(notice[3]) - 1) : (start as number) + count - 1;
    label = `${start}–${end}`;
  }
  // Unknown truncation is not safe even for exact-selection suppression: the
  // host may have returned a different partial selection for identical inputs.
  if ((facts.truncated === true || facts.linesTruncated === true) && !body) return undefined;
  try {
    const absolute = realpathSync(resolve(cwd ?? "", path));
    if (!statSync(absolute).isFile()) return undefined;
    return {
      path: absolute,
      key: JSON.stringify([host, start, limit ?? null]),
      label,
      ...(body ? { body } : {}),
    };
  } catch {
    return undefined;
  }
}
