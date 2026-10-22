import { expect, it } from "bun:test";
import { CallSnapshots } from "../adapters/calls.ts";

it("consumes invocation facts together without colliding across sessions", () => {
  const calls = new CallSnapshots();
  const input = { nested: { path: "original.json" } };
  const a = calls.begin("a", "call", input, "/original");
  a.selection = "off";
  input.nested.path = "changed.json";
  const b = calls.begin("b", "call", {}, "/other");
  b.selection = "builtin";
  expect(calls.take("a", "call")).toMatchObject({
    input: { nested: { path: "original.json" } },
    cwd: "/original",
    selection: "off",
  });
  expect(calls.isCurrent(a)).toBe(false);
  expect(calls.take("a", "call")).toBeUndefined();
  expect(calls.take("b", "call")).toMatchObject({ cwd: "/other", selection: "builtin" });
});

it("invalidates only the affected session, retaining no stale invocation facts", () => {
  const calls = new CallSnapshots();
  const a = calls.begin("a", "call", { secret: "input" }, "/old");
  a.selection = { methods: ["json_compact"] };
  const b = calls.begin("b", "call", {});
  calls.invalidate("a");
  a.selection = "builtin";
  expect(calls.isCurrent(a)).toBe(false);
  expect(calls.isCurrent(b)).toBe(true);
  expect(calls.take("a", "call")).toEqual({ selection: "off" });
  expect(calls.take("a", "unknown")).toBeUndefined();
  calls.invalidate();
  expect(calls.take("b", "call")).toEqual({ selection: "off" });
});

it("does not let an old continuation replace a newer invocation with the same ID", () => {
  const calls = new CallSnapshots();
  const old = calls.begin("session", "call", { path: "old" });
  calls.invalidate();
  const current = calls.begin("session", "call", { path: "new" }, "/new");
  current.selection = "off";
  old.cwd = "/stale";
  old.selection = "builtin";
  expect(calls.isCurrent(old)).toBe(false);
  expect(calls.take("session", "call")).toMatchObject({
    input: { path: "new" },
    cwd: "/new",
    selection: "off",
  });
  const last = calls.begin("session", "last", {});
  calls.clear();
  expect(calls.isCurrent(last)).toBe(false);
});
