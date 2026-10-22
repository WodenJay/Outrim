import { spawn as nodeSpawn } from "node:child_process";
import { nativeReadSelection } from "./reads";
import {
  type AbortSignalLike,
  type BridgeChild,
  createBridge,
  hostOutputFacts,
  inputReferencesOriginals,
  type PolicySelection,
  processingFacts,
  recoveryAttribution,
  type SpawnProcess,
  sourceFor,
  type WrappingState,
} from "./shared";

export const OUTRIM_EXTENSION_MARKER = "outrim-managed: pi-extension";

const BRIDGE_EXECUTABLE = "__OUTRIM_EXECUTABLE__";

interface TextContent {
  type: "text";
  text: string;
  [key: string]: unknown;
}

interface ToolResultEvent {
  toolName: string;
  toolCallId: string;
  parentToolCallId?: string;
  input: Record<string, unknown>;
  content: Array<{ type: string; text?: unknown; [key: string]: unknown }>;
  details?: unknown;
  structuredContent?: unknown;
  isError: boolean;
  usage?: unknown;
}

interface ToolResultContext {
  cwd: string;
  sessionManager: { getSessionId(): string };
  signal: AbortSignalLike | undefined;
}

interface ToolResultEventResult {
  content: TextContent[];
  details: unknown;
  structuredContent: unknown;
  isError: boolean;
  usage: unknown;
}

interface ToolCallEvent {
  toolName: string;
  toolCallId: string;
  parentToolCallId?: string;
  input: Record<string, unknown>;
}

interface PiApi {
  getSettings?(): { shellPath?: string; shellCommandPrefix?: string };
  getAllTools?(): Array<{ name: string; sourceInfo: { source: string; path?: string } }>;
  on(
    event: "tool_call",
    handler: (event: ToolCallEvent, context: ToolResultContext) => Promise<void>,
  ): unknown;
  on(
    event: "tool_result",
    handler: (
      event: ToolResultEvent,
      context: ToolResultContext,
    ) => Promise<ToolResultEventResult | undefined>,
  ): unknown;
  on(
    event: "session_shutdown" | "session_start" | "session_compact" | "session_tree",
    handler: () => void,
  ): unknown;
}

interface PiOptions {
  dialect?: "bash" | "powershell" | "unknown";
  executable?: string;
  spawnProcess?: SpawnProcess;
}

function exitCodeFrom(structuredContent: unknown): number | null {
  if (structuredContent === null || typeof structuredContent !== "object") return null;
  const value = (structuredContent as Record<string, unknown>).exit_code;
  return typeof value === "number" &&
    Number.isInteger(value) &&
    value >= -2_147_483_648 &&
    value <= 2_147_483_647
    ? value
    : null;
}

function isShell(name: string): boolean {
  return ["bash", "powershell", "shell"].includes(name);
}

async function shellDialect(pi: PiApi, name: string): Promise<"bash" | "unknown"> {
  try {
    const tool = pi.getAllTools?.().find((tool) => tool.name === name);
    if (
      name !== "bash" ||
      tool?.sourceInfo.source !== "builtin" ||
      tool.sourceInfo.path !== "builtin:bash"
    )
      return "unknown";
    const settings = pi.getSettings?.();
    if (!settings || settings.shellCommandPrefix) return "unknown";
    // Use the host's own resolver and effective settings, not a second settings-file loader.
    const packageName = "@earendil-works/pi-coding-agent";
    const sdk = await import(packageName);
    const path = sdk.SettingsManager.inMemory(settings).getShellPath();
    const shell = sdk.getShellConfig(path).shell as string;
    return /(?:^|[/\\\\])bash(?:\.exe)?$/i.test(shell) ? "bash" : "unknown";
  } catch {
    return "unknown";
  }
}

const defaultSpawn: SpawnProcess = (executable, args, options) =>
  nodeSpawn(executable, args, options) as unknown as BridgeChild;

export function registerPiExtension(pi: PiApi, options: PiOptions = {}): void {
  const bridge = createBridge(
    "pi",
    options.executable ?? BRIDGE_EXECUTABLE,
    options.spawnProcess ?? defaultSpawn,
  );

  const originalInputs = new Map<string, Record<string, unknown>>();
  const callDirectories = new Map<string, string>();
  const callPolicies = new Map<string, PolicySelection>();
  const callKey = (session: string, call: string) => JSON.stringify([session, call]);
  pi.on("tool_call", async (event, context) => {
    const session = context.sessionManager.getSessionId();
    const key = callKey(session, event.toolCallId);
    originalInputs.set(key, structuredClone(event.input));
    callDirectories.set(key, context.cwd);
    if (!isShell(event.toolName) && !event.parentToolCallId) {
      callPolicies.set(
        key,
        await bridge.selectPolicy(
          session,
          {
            name: event.toolName,
            input: originalInputs.get(key),
            source: sourceFor("pi", event.toolName, event.input),
            exit_code: null,
          },
          context.cwd,
          context.signal,
        ),
      );
    }
    const wrapping = (state: WrappingState, reason: string | null, dialect = "unknown") =>
      bridge.recordWrapping(session, event.toolCallId, event.toolName, state, reason, dialect);
    if (
      !isShell(event.toolName) ||
      event.parentToolCallId ||
      typeof event.input.command !== "string"
    ) {
      wrapping("not_attempted", event.parentToolCallId ? "nested_call" : "source_not_applicable");
      return;
    }
    const original = event.input.command;
    try {
      const dialect = options.dialect ?? (await shellDialect(pi, event.toolName));
      if (dialect === "unknown") {
        wrapping("not_attempted", "unknown_dialect");
        return;
      }
      const command = await bridge.rewrite(
        context.sessionManager.getSessionId(),
        event.toolCallId,
        original,
        dialect,
        context.signal,
        {
          name: event.toolName,
          input: originalInputs.get(key) ?? event.input,
          source: sourceFor("pi", event.toolName, event.input),
          exit_code: null,
        },
      );
      if (command !== undefined && !context.signal?.aborted && event.input.command === original) {
        event.input.command = command;
        wrapping("adopted", null, dialect);
      } else if (command !== undefined) {
        wrapping("withdrawn", context.signal?.aborted ? "cancelled" : "changed_input", dialect);
      }
    } catch {
      // Failure to optimize must not prevent native execution.
    }
  });

  pi.on("tool_result", async (event, context) => {
    // Plugin compress completes a compact, even when it fails.
    if (event.toolName === "compress") bridge.resetReads();
    const key = callKey(context.sessionManager.getSessionId(), event.toolCallId);
    const originalInput = originalInputs.get(key) ?? event.input;
    const cwd = callDirectories.get(key) ?? context.cwd;
    const selection = callPolicies.get(key);
    originalInputs.delete(key);
    callDirectories.delete(key);
    callPolicies.delete(key);
    const attribution = recoveryAttribution(event.toolName, originalInput);
    if (
      attribution &&
      !event.isError &&
      (!isShell(event.toolName) || (exitCodeFrom(event.structuredContent) ?? 0) === 0) &&
      !context.signal?.aborted &&
      !event.parentToolCallId
    ) {
      bridge.recordRecovery(
        context.sessionManager.getSessionId(),
        event.toolCallId,
        event.content.flatMap((part) =>
          part.type === "text" && typeof part.text === "string" ? [part.text] : [],
        ),
        attribution,
      );
    }
    if (isShell(event.toolName)) {
      const text = event.content
        .flatMap((part) =>
          part.type === "text" && typeof part.text === "string" ? [part.text] : [],
        )
        .join("\n");
      bridge.recordObservation(
        {
          session: context.sessionManager.getSessionId(),
          toolCallId: event.toolCallId,
          diagnostics: hostOutputFacts("pi", event.details),
          tool: {
            name: event.toolName,
            input: null,
            source: "unknown",
            exit_code: exitCodeFrom(event.structuredContent),
          },
        },
        text,
        null,
        {
          outcome: context.signal?.aborted ? "cancelled" : "unsupported",
          reason: event.isError ? "host_error" : "shell_result_observed",
          metadataOnly: true,
        },
      );
      const [content] = event.content;
      if (
        event.isError ||
        hostOutputFacts("pi", event.details).completeness === "host_truncated" ||
        event.parentToolCallId ||
        event.content.length !== 1 ||
        content?.type !== "text" ||
        typeof content.text !== "string" ||
        inputReferencesOriginals(originalInput)
      )
        return undefined;
      const processed = await bridge.process(
        context.sessionManager.getSessionId(),
        {
          name: event.toolName,
          input: originalInput,
          source: "unknown",
          exit_code: exitCodeFrom(event.structuredContent),
        },
        content.text,
        context.signal,
        cwd,
        selection,
        { call: event.toolCallId },
      );
      if (!processed || context.signal?.aborted || processed.reason === "unverified_shell_read")
        return undefined;
      bridge.recordDelivery(content.text, processed, context.signal);
      if (processed.text === content.text) return undefined;
      return {
        content: [{ ...content, type: "text", text: processed.text }],
        details: event.details,
        structuredContent: event.structuredContent,
        isError: event.isError,
        usage: event.usage,
      };
    }
    const observation = {
      session: context.sessionManager.getSessionId(),
      toolCallId: event.toolCallId ?? null,
      diagnostics: hostOutputFacts("pi", event.details),
      tool: {
        name: event.toolName,
        input: originalInput,
        source: sourceFor("pi", event.toolName, originalInput),
        exit_code: exitCodeFrom(event.structuredContent),
      },
    };
    const [content] = event.content;
    if (
      event.parentToolCallId ||
      event.content.length !== 1 ||
      content?.type !== "text" ||
      typeof content.text !== "string"
    ) {
      const texts = event.content.flatMap((part) =>
        part.type === "text" && typeof part.text === "string" ? [part.text] : [],
      );
      const text = texts.length ? texts.join("\n") : null;
      bridge.recordObservation(observation, text, text, {
        outcome: "unsupported",
        reason: event.parentToolCallId ? "nested_tool_call" : "unsupported_content_shape",
      });
      return undefined;
    }

    const original = content as TextContent;
    const text = original.text;
    if (inputReferencesOriginals(originalInput)) {
      bridge.recordObservation(observation, text, text, {
        outcome: context.signal?.aborted ? "cancelled" : "recovery_skipped",
        reason: "original_directory_in_arguments",
      });
      bridge.recordDelivery(text, "recovery_skipped", context.signal);
      return undefined;
    }

    const processed = await bridge.process(
      observation.session,
      observation.tool,
      text,
      context.signal,
      cwd,
      selection,
      event.toolName === "read" &&
        !event.isError &&
        observation.diagnostics.completeness !== "host_truncated"
        ? (() => {
            const selection = nativeReadSelection("pi", originalInput, text, cwd);
            return selection ? { selection } : undefined;
          })()
        : undefined,
    );
    bridge.recordObservation(
      observation,
      text,
      context.signal?.aborted ? text : (processed?.text ?? text),
      {
        ...processingFacts([processed]),
        ...(context.signal?.aborted ? { outcome: "cancelled", reason: "host_signal_aborted" } : {}),
      },
    );
    if (processed === undefined) {
      bridge.recordDelivery(text, undefined, context.signal);
      return undefined;
    }
    if (context.signal?.aborted) return undefined;

    bridge.recordDelivery(text, processed, context.signal);
    if (processed.text === text) return undefined;

    return {
      content: [{ ...original, text: processed.text }],
      details: event.details,
      structuredContent: event.structuredContent,
      isError: event.isError,
      usage: event.usage,
    };
  });

  for (const event of ["session_start", "session_compact", "session_tree"] as const) {
    pi.on(event, () => {
      originalInputs.clear();
      bridge.resetReads();
    });
  }
  pi.on("session_shutdown", () => bridge.close());
}

export default function piExtension(pi: PiApi): void {
  registerPiExtension(pi);
}
