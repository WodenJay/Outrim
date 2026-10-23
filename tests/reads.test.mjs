import { expect, test } from "bun:test";
import { nativeReadSelection } from "../adapters/reads";

const path = "adapters/reads.ts";
const cwd = process.cwd();
function body(host, text, input = {}, facts = {}) {
  return nativeReadSelection(
    host,
    { [host === "pi" ? "path" : "filePath"]: path, ...input },
    text,
    cwd,
    facts,
  )?.body;
}
function reconstructed(value) {
  return (
    (value.prefix ?? "") + value.lines.map((line) => line.rendered).join("") + (value.suffix ?? "")
  );
}
test("Pi indexes only proven complete source lines without changing CRLF or Unicode", () => {
  const text = "α\r\nβ\nlast";
  const value = body("pi", text, { offset: 7 });
  expect(value).toMatchObject({
    start: 7,
    lines: [
      { text: "α\r\n", rendered: "α\r\n" },
      { text: "β\n", rendered: "β\n" },
    ],
    suffix: "last",
  });
  expect(reconstructed(value)).toBe(text);
});
test("OpenCode retains headings and strips realistic sequential numbering only from source evidence", () => {
  const text = "Read file source.ts, lines 7-9\n7: α\r\n8: β\n9: last";
  const value = body("opencode", text, { offset: 7 });
  expect(value.lines).toEqual([
    { text: "α\r\n", rendered: "7: α\r\n" },
    { text: "β\n", rendered: "8: β\n" },
  ]);
  expect(reconstructed(value)).toBe(text);
});
test("Pi pagination notice remains outside evidence, including ambiguous last source line", () => {
  const text = "one\ntwo\n\n[Showing lines 1-2 of 8. Use offset=3 to continue.]";
  const value = body("pi", text);
  expect(value.lines).toEqual([{ text: "one\n", rendered: "one\n" }]);
  expect(value.suffix).toBe("two\n\n[Showing lines 1-2 of 8. Use offset=3 to continue.]");
  expect(reconstructed(value)).toBe(text);
});
test("uncertain truncation and oversized first lines provide no range evidence", () => {
  expect(
    body("pi", "partial", {}, { truncated: true, firstLineExceedsLimit: true }),
  ).toBeUndefined();
  expect(body("pi", "one\ntwo", {}, { truncated: true })).toBeUndefined();
  expect(
    body(
      "opencode",
      "Read file source.ts, lines 1-2\n1: partial\n2: last",
      {},
      { linesTruncated: true },
    ),
  ).toBeUndefined();
});
test("Pi explicit-limit and byte-limit notices preserve the final source line", () => {
  for (const [notice, facts] of [
    ["[6 more lines in file. Use offset=3 to continue.]", {}],
    [
      "[Showing lines 1-2 of 8 (50.0KB limit). Use offset=3 to continue.]",
      { truncated: true, lastLinePartial: true },
    ],
  ]) {
    const text = `one\ntwo\n\n${notice}`;
    const value = body("pi", text, {}, facts);
    expect(value.lines).toEqual([{ text: "one\n", rendered: "one\n" }]);
    expect(reconstructed(value)).toBe(text);
  }
});
test("ambiguous final and first partial lines never become evidence", () => {
  for (const text of ["only", "\n[Showing lines 1-1 of 8. Use offset=2 to continue.]"]) {
    expect(body("pi", text)).toBeUndefined();
  }
  expect(
    body(
      "pi",
      "partial\n\n[Showing lines 1-1 of 8. Use offset=2 to continue.]",
      {},
      { truncated: true, firstLineExceedsLimit: true },
    ),
  ).toBeUndefined();
  expect(body("opencode", "Read file source.ts, lines 1-1\n1: only")).toBeUndefined();
});
test("unknown notices and inconsistent pagination are not range evidence", () => {
  expect(
    body("pi", "one\ntwo\n\n[Showing lines 1-2 of 8. Use offset=9 to continue.]"),
  ).toBeUndefined();
  expect(
    body("opencode", "Read file source.ts, lines 1-2\n1: one\n2: two\n[Unknown notice]"),
  ).toBeUndefined();
  expect(
    body(
      "opencode",
      "Read file source.ts, lines 1-2\n1: one\n2: two\n[Output truncated. Continue reading with offset: 9]",
    ),
  ).toBeUndefined();
});
test("nonsequential OpenCode numbering falls back to exact selection", () => {
  expect(body("opencode", "Read file source.ts, lines 1-2\n1: one\n1: two")).toBeUndefined();
});
