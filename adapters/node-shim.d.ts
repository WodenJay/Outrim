declare module "node:fs" {
  export function readFileSync(path: string, encoding: "utf8"): string;
}
declare module "node:os" {
  export function homedir(): string;
}
declare module "node:path" {
  export function join(...parts: string[]): string;
}

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
