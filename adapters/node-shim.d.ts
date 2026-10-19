declare class AbortSignal {
  readonly aborted: boolean;
  addEventListener(type: "abort", listener: () => void, options?: { once?: boolean }): void;
  removeEventListener(type: "abort", listener: () => void): void;
}

declare class AbortController {
  readonly signal: AbortSignal;
  abort(): void;
}

declare module "node:child_process" {
  export function spawn(
    executable: string,
    args: string[],
    options: {
      stdio: ["pipe", "pipe", "ignore"];
      windowsHide: boolean;
    },
  ): unknown;
}
