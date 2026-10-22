import { expect, it } from "bun:test";
import { EventEmitter } from "node:events";
import { createTransport } from "../adapters/bridge/transport.ts";

class Child extends EventEmitter {
  writes = [];
  killed = false;
  ended = false;
  stdin = new EventEmitter();
  stdout = new EventEmitter();
  constructor() {
    super();
    this.stdin.write = (line, callback) => {
      this.writes.push(JSON.parse(line));
      callback();
      return true;
    };
    this.stdin.end = () => {
      this.ended = true;
    };
    this.stdout.setEncoding = () => {};
  }
  kill() {
    this.killed = true;
    return true;
  }
  line(index, fields = {}) {
    return `${JSON.stringify({ version: 2, id: this.writes[index].id, ...fields })}\n`;
  }
}

function harness() {
  const children = [];
  const transport = createTransport(
    "outrim",
    ["bridge", "--recording", "off"],
    (executable, args, options) => {
      expect(executable).toBe("outrim");
      expect(args).toEqual(["bridge", "--recording", "off"]);
      expect(options).toEqual({ stdio: ["pipe", "pipe", "ignore"], windowsHide: true });
      const child = new Child();
      children.push(child);
      return child;
    },
  );
  return { children, transport };
}

it("frames split and combined replies and correlates out-of-order requests", async () => {
  const { children, transport } = harness();
  expect(transport.running).toBe(false);
  const a = transport.exchange({ type: "process_parts", request: { id: "", tool: {} } });
  const b = transport.exchange({ type: "resolve_policy" });
  const child = children[0];
  expect(child.writes[0].request.id).toBe(child.writes[0].id);
  const replies = child.line(1, { selection: "off" }) + child.line(0, { parts: [] });
  child.stdout.emit("data", replies.slice(0, 11));
  child.stdout.emit("data", replies.slice(11));
  expect(await a).toMatchObject({ id: child.writes[0].id, parts: [] });
  expect(await b).toMatchObject({ id: child.writes[1].id, selection: "off" });
  transport.close();
});

it.each([
  "not json\n",
  "[]\n",
  "null\n",
  '{"version":1,"id":"1"}\n',
  '{"version":2,"id":"unknown"}\n',
])(
  "fails open on invalid framing %s without letting old child events damage the restart",
  async (line) => {
    const { children, transport } = harness();
    const pending = transport.exchange({});
    const old = children[0];
    old.stdout.emit("data", line);
    expect(await pending).toBeUndefined();
    expect(old.killed).toBe(true);
    const next = transport.exchange({});
    const current = children[1];
    old.emit("exit", 1, null);
    old.stdout.emit("data", old.line(0));
    current.stdout.emit("data", current.line(0, { text: "healthy" }));
    expect(await next).toMatchObject({ text: "healthy" });
    transport.close();
  },
);

it("cancels only one request and tolerates its late reply on the same child", async () => {
  const { children, transport } = harness();
  const controller = new AbortController();
  const cancelled = transport.exchange({}, controller.signal);
  const child = children[0];
  controller.abort();
  expect(await cancelled).toBeUndefined();
  expect(child.killed).toBe(false);
  const next = transport.exchange({});
  child.stdout.emit("data", child.line(0) + child.line(1, { text: "next" }));
  expect(await next).toMatchObject({ text: "next" });
  expect(children).toHaveLength(1);
  transport.close();
});

it("fails pending requests on a write error and closes gracefully after restart", async () => {
  const { children, transport } = harness();
  const failed = transport.exchange({});
  children[0].stdin.emit("error", new Error("broken pipe"));
  expect(await failed).toBeUndefined();
  const pending = transport.exchange({});
  transport.close();
  expect(await pending).toBeUndefined();
  expect(children[1].ended).toBe(true);
  expect(await transport.exchange({})).toBeUndefined();
  expect(children).toHaveLength(2);
});

it("times out a stalled child only when the caller requests a deadline", async () => {
  const { children, transport } = harness();
  const ordinary = transport.exchange({});
  const timed = transport.exchange({}, undefined, 5);
  expect(await timed).toBeUndefined();
  expect(await ordinary).toBeUndefined();
  expect(children[0].killed).toBe(true);
  transport.close();
});
