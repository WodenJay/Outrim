import { spawn as nodeSpawn } from "node:child_process";
import {
  createBridge,
  inputReferencesOriginals,
  sourceFor,
  type AbortSignalLike,
  type BridgeChild,
  type SpawnProcess,
} from "./shared";

const BRIDGE_EXECUTABLE = "__OUTRIM_EXECUTABLE__";

interface ToolContent {
  type: string;
  text?: unknown;
  [key: string]: unknown;
}

interface ToolResult {
  content?: string | readonly unknown[];
  output?: unknown;
  metadata?: unknown;
  [key: string]: unknown;
}

interface ToolHookBeforeEvent {
  tool: string;
  sessionID: string;
  id: string;
}

export interface ToolHookEvent {
  status: string;
  tool: string;
  sessionID: string;
  id: string;
  input: unknown;
  result?: ToolResult;
}

interface ToolHookRegistration {
  dispose(): void | Promise<void>;
}

interface OpenCodeEvent {
  type?: unknown;
  properties?: unknown;
}

export interface OpenCodePluginContext {
  tool: {
    hook(
      name: "execute.before",
      handler: (event: ToolHookBeforeEvent) => Promise<void> | void,
    ): Promise<ToolHookRegistration>;
    hook(
      name: "execute.after",
      handler: (event: ToolHookEvent) => Promise<void> | void,
    ): Promise<ToolHookRegistration>;
  };
  event?: {
    subscribe(options?: { signal?: AbortSignalLike }): AsyncIterable<OpenCodeEvent>;
  };
}

interface OpenCodeOptions {
  executable?: string;
  spawnProcess?: SpawnProcess;
}

interface TextSelection {
  texts: string[];
  apply(replacements: Array<string | undefined>): ToolResult["content"];
}

const defaultSpawn: SpawnProcess = (executable, args, options) =>
  nodeSpawn(executable, args, options) as unknown as BridgeChild;

function record(value: unknown): Record<string, unknown> | undefined {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : undefined;
}

function exitCodeFrom(metadata: unknown): number | null {
  const value = record(metadata);
  const code = value?.exit;
  return typeof code === "number" && Number.isInteger(code) &&
      code >= -2_147_483_648 && code <= 2_147_483_647
    ? code
    : null;
}

function textSelection(result: ToolResult): TextSelection | undefined {
  if (typeof result.content === "string") {
    const original = result.content;
    return { texts: [original], apply: ([text]) => text ?? original };
  }

  if (Array.isArray(result.content)) {
    const content = result.content;
    const texts = content.flatMap((part) => {
      const item = record(part) as ToolContent | undefined;
      return item?.type === "text" && typeof item.text === "string" ? [item.text] : [];
    });
    if (texts.length > 0) {
      return {
        texts,
        apply: (replacements) => {
          let index = 0;
          return content.map((part) => {
            const item = record(part) as ToolContent | undefined;
            if (item?.type !== "text" || typeof item.text !== "string") return part;
            const text = replacements[index++];
            return typeof text === "string" ? { ...item, text } : part;
          });
        },
      };
    }
    if (content.length !== 0 || typeof result.output !== "string") return undefined;
  } else if (result.content !== undefined || typeof result.output !== "string") {
    return undefined;
  }

  const original = result.output as string;
  return { texts: [original], apply: ([text]) => text ?? original };
}

function sessionIdFrom(event: OpenCodeEvent): string | undefined {
  const properties = record(event.properties);
  if (typeof properties?.sessionID === "string") return properties.sessionID;
  const info = record(properties?.info);
  return typeof info?.id === "string" ? info.id : undefined;
}

export async function registerOpenCodePlugin(
  context: OpenCodePluginContext,
  options: OpenCodeOptions = {},
): Promise<() => Promise<void>> {
  const bridge = createBridge(
    "opencode",
    options.executable ?? BRIDGE_EXECUTABLE,
    options.spawnProcess ?? defaultSpawn,
  );
  const activeCallsBySession = new Map<string, Set<AbortController>>();
  // OpenCode v2 Code Mode forwards the wrapper call ID to nested tool hooks.
  const activeCodeModeCalls = new Map<string, string>();
  const eventController = new AbortController();
  let closed = false;

  function abortSession(session: string): void {
    for (const controller of activeCallsBySession.get(session) ?? []) controller.abort();
  }

  async function handleResult(event: ToolHookEvent): Promise<void> {
    const isCodeModeCall = activeCodeModeCalls.get(event.id) === event.sessionID;
    if (event.tool === "execute") activeCodeModeCalls.delete(event.id);
    if (
      closed || event.status !== "completed" ||
      (isCodeModeCall && event.tool !== "execute")
    ) return;
    const input = record(event.input);
    const result = event.result;
    if (!input || !result || inputReferencesOriginals(input)) return;

    const selection = textSelection(result);
    if (!selection) return;

    const controller = new AbortController();
    let sessionCalls = activeCallsBySession.get(event.sessionID);
    if (!sessionCalls) {
      sessionCalls = new Set();
      activeCallsBySession.set(event.sessionID, sessionCalls);
    }
    sessionCalls.add(controller);
    try {
      const replacements = await Promise.all(
        selection.texts.map((text) =>
          bridge.process(
            event.sessionID,
            {
              name: event.tool,
              input,
              source: sourceFor("opencode", event.tool, input),
              exit_code: exitCodeFrom(result.metadata),
            },
            text,
            controller.signal,
          ),
        ),
      );
      if (closed || controller.signal.aborted) return;
      if (
        replacements.some((text, index) => text !== undefined && text !== selection.texts[index])
      ) {
        event.result = { ...result, content: selection.apply(replacements) };
      }
    } finally {
      sessionCalls.delete(controller);
      if (sessionCalls.size === 0) activeCallsBySession.delete(event.sessionID);
    }
  }

  const beforeRegistration = await context.tool.hook("execute.before", (event) => {
    if (event.tool === "execute") activeCodeModeCalls.set(event.id, event.sessionID);
  });
  let registration: ToolHookRegistration;
  try {
    registration = await context.tool.hook("execute.after", handleResult);
  } catch (error) {
    await beforeRegistration.dispose();
    throw error;
  }
  let eventLoop: Promise<void> | undefined;
  if (context.event) {
    eventLoop = (async () => {
      try {
        for await (const event of context.event!.subscribe({ signal: eventController.signal })) {
          if (event.type !== "session.deleted" && event.type !== "session.execution.interrupted") {
            continue;
          }
          const session = sessionIdFrom(event);
          if (session) abortSession(session);
        }
      } catch {
        // Event transport failure must not disable result processing.
      }
    })();
  }

  return async () => {
    if (closed) return;
    closed = true;
    eventController.abort();
    for (const sessionCalls of activeCallsBySession.values()) {
      for (const controller of sessionCalls) controller.abort();
    }
    bridge.close();
    activeCodeModeCalls.clear();
    await registration.dispose();
    await beforeRegistration.dispose();
    await eventLoop;
  };
}
