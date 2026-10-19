import { spawn as nodeSpawn } from "node:child_process";
import {
  createBridge,
  inputReferencesOriginals,
  type AbortSignalLike,
  type BridgeChild,
  type Source,
  type SpawnProcess,
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

const TEST_COMMANDS = [
  /^\s*cargo\s+test\b/i,
  /^\s*(?:bun\s+(?:test|run\s+test)|npm\s+(?:test|run\s+test)|pnpm\s+(?:test|run\s+test)|yarn\s+(?:test|run\s+test))\b/i,
  /^\s*(?:pytest|go\s+test|jest|vitest)\b/i,
];

const BUILD_COMMANDS = [
  /^\s*cargo\s+(?:build|check|clippy)\b/i,
  /^\s*(?:bun|npm|pnpm|yarn)\s+(?:build|run\s+build)\b/i,
  /^\s*(?:go\s+build|tsc|vite\s+build|cmake\s+--build|make)\b/i,
];

const RUNTIME_COMMANDS = [
  /^\s*cargo\s+run\b/i,
  /^\s*(?:bun|npm|pnpm|yarn)\s+(?:start|run\s+(?:dev|start))\b/i,
  /^\s*(?:node|python(?:3(?:\.\d+)?)?|go)\s+(?:\S+|$)/i,
];

function matchesAny(patterns: RegExp[], command: string): boolean {
  return patterns.some((pattern) => pattern.test(command));
}

function sourceFor(toolName: string, input: Record<string, unknown>): Source {
  if (["find", "grep", "ls"].includes(toolName)) return "search";
  if (toolName !== "bash" && toolName !== "powershell") return "unknown";
  const command = input.command;
  if (typeof command !== "string") return "unknown";
  if (matchesAny(TEST_COMMANDS, command)) return "test_log";
  if (matchesAny(BUILD_COMMANDS, command)) return "build_log";
  if (matchesAny(RUNTIME_COMMANDS, command)) return "runtime_log";
  return "unknown";
}

function exitCodeFrom(structuredContent: unknown): number | null {
  if (structuredContent === null || typeof structuredContent !== "object") return null;
  const value = (structuredContent as Record<string, unknown>).exit_code;
  return typeof value === "number" && Number.isInteger(value) &&
      value >= -2_147_483_648 && value <= 2_147_483_647
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
    if (
      event.parentToolCallId ||
      event.content.length !== 1 ||
      event.content[0].type !== "text" ||
      typeof event.content[0].text !== "string" ||
      inputReferencesOriginals(event.input)
    ) {
      return undefined;
    }

    const original = event.content[0] as TextContent;
    const text = original.text;
    const processed = await bridge.process(
      context.sessionManager.getSessionId(),
      {
        name: event.toolName,
        input: event.input,
        source: sourceFor(event.toolName, event.input),
        exit_code: exitCodeFrom(event.structuredContent),
      },
      text,
      context.signal,
    );
    if (processed === undefined || processed === text) return undefined;

    return {
      content: [{ ...original, text: processed }],
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
