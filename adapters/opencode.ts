import { spawn as nodeSpawn } from "node:child_process";
import {
  type AbortSignalLike,
  type BridgeChild,
  createBridge,
  hostOutputFacts,
  inputReferencesOriginals,
  processingFacts,
  recoveryAttribution,
  type SpawnProcess,
  sourceFor,
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
  agent: string;
  input: unknown;
}

type Dialect = "bash" | "powershell" | "unknown";

interface SessionContextEvent {
  sessionID: string;
  agent: string;
  tools: Record<string, { description: string }>;
}

interface ShellBeforeEvent {
  command: string;
  shell: string;
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
  session?: {
    hook(
      name: "context" | "generate" | "compaction",
      handler: (event: SessionContextEvent) => void,
    ): Promise<ToolHookRegistration>;
  };
  shell?: {
    hook(
      name: "create.before",
      handler: (event: ShellBeforeEvent) => void,
    ): Promise<ToolHookRegistration>;
  };
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
  apply(replacements: Array<string | undefined>): string | readonly unknown[];
}

const defaultSpawn: SpawnProcess = (executable, args, options) =>
  nodeSpawn(executable, args, options) as unknown as BridgeChild;

function record(value: unknown): Record<string, unknown> | undefined {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;
}

function isShell(tool: string): boolean {
  return ["shell", "bash", "powershell"].includes(tool);
}

function shellDialect(shell: string): Dialect {
  const name = shell
    .split(/[/\\\\]/)
    .pop()
    ?.toLowerCase()
    .replace(/\.exe$/, "");
  if (name === "bash") return "bash";
  if (name === "pwsh" || name === "powershell") return "powershell";
  return "unknown";
}

function exitCodeFrom(metadata: unknown): number | null {
  const value = record(metadata);
  const code = value?.exit;
  return typeof code === "number" &&
    Number.isInteger(code) &&
    code >= -2_147_483_648 &&
    code <= 2_147_483_647
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
  const originalInputs = new Map<string, unknown>();
  const callKey = (session: string, call: string) => JSON.stringify([session, call]);
  const activeCallsBySession = new Map<string, Set<AbortController>>();
  // OpenCode v2 Code Mode forwards the wrapper call ID to nested tool hooks.
  const activeCodeModeCalls = new Map<string, Map<string, number>>();
  const dialects = new Map<string, Map<string, Dialect>>();
  const rewrites = new Map<
    string,
    {
      original: string;
      dialect: Dialect;
      session: string;
      call: string;
    }
  >();
  const eventController = new AbortController();
  let closed = false;

  function abortSession(session: string): void {
    for (const controller of activeCallsBySession.get(session) ?? []) controller.abort();
    activeCodeModeCalls.delete(session);
    dialects.delete(session);
    for (const [command, rewrite] of rewrites) {
      if (rewrite.session === session) rewrites.delete(command);
    }
  }

  async function handleBefore(event: ToolHookBeforeEvent): Promise<void> {
    if (closed) return;
    const originalInput = record(event.input);
    originalInputs.set(
      callKey(event.sessionID, event.id),
      originalInput ? { ...originalInput } : event.input,
    );
    if (event.tool === "execute") {
      let sessionCalls = activeCodeModeCalls.get(event.sessionID);
      if (!sessionCalls) {
        sessionCalls = new Map();
        activeCodeModeCalls.set(event.sessionID, sessionCalls);
      }
      sessionCalls.set(event.id, (sessionCalls.get(event.id) ?? 0) + 1);
      return;
    }
    const input = record(event.input);
    const dialect = dialects.get(event.sessionID)?.get(event.agent) ?? "unknown";
    if (
      event.tool !== "shell" ||
      !context.shell ||
      dialect === "unknown" ||
      typeof input?.command !== "string" ||
      activeCodeModeCalls.get(event.sessionID)?.has(event.id)
    ) {
      bridge.recordWrapping(
        event.sessionID,
        event.id,
        event.tool,
        "not_attempted",
        dialect === "unknown" ? "unknown_dialect" : "source_not_applicable",
        dialect,
      );
      return;
    }
    const original = input.command;
    const controller = new AbortController();
    let calls = activeCallsBySession.get(event.sessionID);
    if (!calls) {
      calls = new Set();
      activeCallsBySession.set(event.sessionID, calls);
    }
    calls.add(controller);
    try {
      const command = await bridge.rewrite(
        event.sessionID,
        event.id,
        original,
        dialect,
        controller.signal,
      );
      if (
        closed ||
        controller.signal.aborted ||
        input.command !== original ||
        !command ||
        command === original
      ) {
        if (command)
          bridge.recordWrapping(
            event.sessionID,
            event.id,
            event.tool,
            "withdrawn",
            controller.signal.aborted ? "cancelled" : "changed_input",
            dialect,
          );
        return;
      }
      bridge.recordWrapping(event.sessionID, event.id, event.tool, "adopted", null, dialect);
      rewrites.set(command, { original, dialect, session: event.sessionID, call: event.id });
      input.command = command;
    } finally {
      calls.delete(controller);
      if (calls.size === 0) activeCallsBySession.delete(event.sessionID);
    }
  }

  async function handleResult(event: ToolHookEvent): Promise<void> {
    const codeModeSessionCalls = activeCodeModeCalls.get(event.sessionID);
    const codeModeDepth = codeModeSessionCalls?.get(event.id) ?? 0;
    const isCodeModeCall = codeModeDepth > 0;
    if (event.tool === "execute" && codeModeSessionCalls && isCodeModeCall) {
      if (codeModeDepth === 1) {
        codeModeSessionCalls.delete(event.id);
        if (codeModeSessionCalls.size === 0) activeCodeModeCalls.delete(event.sessionID);
      } else {
        codeModeSessionCalls.set(event.id, codeModeDepth - 1);
      }
    }
    for (const [command, rewrite] of rewrites) {
      if (rewrite.session === event.sessionID && rewrite.call === event.id)
        rewrites.delete(command);
    }
    if (closed) return;
    const key = callKey(event.sessionID, event.id);
    const recoveryInput = originalInputs.get(key) ?? event.input;
    originalInputs.delete(key);
    const recoverySelection = event.result ? textSelection(event.result) : undefined;
    const attribution = recoveryAttribution(event.tool, recoveryInput);
    if (
      attribution &&
      event.status === "completed" &&
      recoverySelection &&
      !isCodeModeCall &&
      (exitCodeFrom(event.result?.metadata) ?? 0) === 0
    ) {
      bridge.recordRecovery(event.sessionID, event.id, recoverySelection.texts, attribution);
    }
    if (isShell(event.tool)) {
      bridge.recordObservation(
        {
          session: event.sessionID,
          toolCallId: event.id,
          diagnostics: hostOutputFacts("opencode", event.result?.metadata),
          tool: {
            name: event.tool,
            input: null,
            source: "unknown",
            exit_code: exitCodeFrom(event.result?.metadata),
          },
        },
        recoverySelection?.texts.join("\n") ?? null,
        null,
        {
          outcome: event.status === "cancelled" ? "cancelled" : "unsupported",
          reason: event.status === "completed" ? "shell_result_observed" : "host_error",
          metadataOnly: true,
        },
      );
      return;
    }
    const input = record(event.input);
    const result = event.result;
    const selection = result ? textSelection(result) : undefined;
    const observation = {
      session: event.sessionID ?? null,
      toolCallId: event.id ?? null,
      diagnostics: hostOutputFacts("opencode", result?.metadata),
      tool: {
        name: event.tool,
        input: event.input ?? null,
        source: sourceFor("opencode", event.tool, input ?? {}),
        exit_code: exitCodeFrom(result?.metadata),
      },
    };
    const originalText = selection ? selection.texts.join("\n") : null;
    const unsupportedReason =
      event.status !== "completed"
        ? `host_status:${event.status}`
        : isCodeModeCall && (event.tool !== "execute" || codeModeDepth > 1)
          ? "code_mode_nested"
          : !input
            ? "unsupported_arguments"
            : !result
              ? "missing_result"
              : !selection
                ? "unsupported_content_shape"
                : null;
    if (unsupportedReason || !input || !result || !selection) {
      bridge.recordObservation(observation, originalText, originalText, {
        outcome: event.status === "cancelled" ? "cancelled" : "unsupported",
        reason: unsupportedReason,
      });
      return;
    }
    if (inputReferencesOriginals(input)) {
      bridge.recordObservation(observation, originalText, originalText, {
        outcome: "recovery_skipped",
        reason: "original_directory_in_arguments",
      });
      bridge.recordDelivery(selection.texts, "recovery_skipped");
      return;
    }

    const controller = new AbortController();
    let bridgeSessionCalls = activeCallsBySession.get(event.sessionID);
    if (!bridgeSessionCalls) {
      bridgeSessionCalls = new Set();
      activeCallsBySession.set(event.sessionID, bridgeSessionCalls);
    }
    bridgeSessionCalls.add(controller);
    try {
      const processedResults = await Promise.all(
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
      if (closed || controller.signal.aborted) {
        bridge.recordObservation(observation, originalText, originalText, {
          ...processingFacts(processedResults),
          outcome: "cancelled",
          reason: "host_signal_aborted",
        });
        return;
      }
      selection.texts.forEach((original, index) => {
        bridge.recordDelivery(original, processedResults[index], controller.signal);
      });
      const replacements = processedResults.map((processed) => processed?.text);
      bridge.recordObservation(
        observation,
        originalText,
        selection.texts.map((text, index) => replacements[index] ?? text).join("\n"),
        processingFacts(processedResults),
      );
      if (
        replacements.some((text, index) => text !== undefined && text !== selection.texts[index])
      ) {
        event.result = { ...result, content: selection.apply(replacements) };
      }
    } finally {
      bridgeSessionCalls.delete(controller);
      if (bridgeSessionCalls.size === 0) activeCallsBySession.delete(event.sessionID);
    }
  }

  const registrations: ToolHookRegistration[] = [];
  try {
    if (context.session && context.shell) {
      // v2.0.26 supplies the resolved agent shell in this host-generated sentence.
      // It is only a hint: verify against the actual launch shell below.
      const observeShell = (event: SessionContextEvent): void => {
        const description = event.tools.shell?.description ?? "";
        const match =
          /(?:^| )Commands run on (?:Windows|macOS|Linux) using ([a-zA-Z0-9_-]+)\./.exec(
            description,
          );
        let agents = dialects.get(event.sessionID);
        if (!agents) {
          agents = new Map();
          dialects.set(event.sessionID, agents);
        }
        agents.set(event.agent, match?.[1] ? shellDialect(match[1]) : "unknown");
      };
      for (const name of ["context", "generate", "compaction"] as const) {
        registrations.push(await context.session.hook(name, observeShell));
      }
      registrations.push(
        await context.shell.hook("create.before", (event) => {
          const rewrite = rewrites.get(event.command);
          if (!rewrite) return;
          rewrites.delete(event.command);
          const actual = shellDialect(event.shell);
          if (actual !== rewrite.dialect) {
            event.command = rewrite.original;
            bridge.recordWrapping(
              rewrite.session,
              rewrite.call,
              "shell",
              "withdrawn",
              "dialect_mismatch",
              rewrite.dialect,
              actual,
            );
          } else {
            bridge.recordWrapping(
              rewrite.session,
              rewrite.call,
              "shell",
              "adopted",
              null,
              rewrite.dialect,
              actual,
            );
          }
        }),
      );
    }
    registrations.push(await context.tool.hook("execute.before", handleBefore));
    registrations.push(await context.tool.hook("execute.after", handleResult));
  } catch (error) {
    for (const registration of registrations.reverse()) await registration.dispose();
    bridge.close();
    throw error;
  }
  let eventLoop: Promise<void> | undefined;
  const events = context.event;
  if (events) {
    eventLoop = (async () => {
      try {
        for await (const event of events.subscribe({ signal: eventController.signal })) {
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
    dialects.clear();
    rewrites.clear();
    originalInputs.clear();
    for (const registration of registrations.reverse()) await registration.dispose();
    await eventLoop;
  };
}
