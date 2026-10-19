import { spawn as nodeSpawn } from "node:child_process";
import {
  type AbortSignalLike,
  type BridgeChild,
  createBridge,
  inputReferencesOriginals,
  processingFacts,
  type SpawnProcess,
  sourceFor,
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

interface PiApi {
  on(
    event: "tool_result",
    handler: (
      event: ToolResultEvent,
      context: ToolResultContext,
    ) => Promise<ToolResultEventResult | undefined>,
  ): unknown;
  on(event: "session_shutdown", handler: () => void): unknown;
}

interface PiOptions {
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

const defaultSpawn: SpawnProcess = (executable, args, options) =>
  nodeSpawn(executable, args, options) as unknown as BridgeChild;

export function registerPiExtension(pi: PiApi, options: PiOptions = {}): void {
  const bridge = createBridge(
    "pi",
    options.executable ?? BRIDGE_EXECUTABLE,
    options.spawnProcess ?? defaultSpawn,
  );

  pi.on("tool_result", async (event, context) => {
    const observation = {
      session: context.sessionManager.getSessionId(),
      toolCallId: event.toolCallId ?? null,
      tool: {
        name: event.toolName,
        input: event.input,
        source: sourceFor("pi", event.toolName, event.input),
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
      const texts = event.content.flatMap((part) => part.type === "text" && typeof part.text === "string" ? [part.text] : []);
      const text = texts.length ? texts.join("\n") : null;
      bridge.recordObservation(observation, text, text, {
        outcome: "unsupported",
        reason: event.parentToolCallId ? "nested_tool_call" : "unsupported_content_shape",
      });
      return undefined;
    }

    const original = content as TextContent;
    const text = original.text;
    if (inputReferencesOriginals(event.input)) {
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
    );
    bridge.recordObservation(observation, text, context.signal?.aborted ? text : processed?.text ?? text, {
      ...processingFacts([processed]),
      ...(context.signal?.aborted ? { outcome: "cancelled", reason: "host_signal_aborted" } : {}),
    });
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

  pi.on("session_shutdown", () => bridge.close());
}

export default function piExtension(pi: PiApi): void {
  registerPiExtension(pi);
}
