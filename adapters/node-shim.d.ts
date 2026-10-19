declare class AbortController {
  readonly signal: import("./shared").AbortSignalLike;
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
