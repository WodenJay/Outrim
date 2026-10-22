import type { PolicySelection } from "./shared";

interface CallFacts<Input> {
  input?: Input;
  cwd?: string | undefined;
  selection?: PolicySelection;
}

interface CallSnapshot<Input> extends CallFacts<Input> {
  readonly session: string;
  readonly call: string;
  readonly input: Input;
}

/** Invocation facts are saved, consumed and invalidated together. */
export class CallSnapshots<Input> {
  private readonly sessions = new Map<string, Map<string, CallSnapshot<Input> | null>>();

  begin(session: string, call: string, input: Input, cwd?: string): CallSnapshot<Input> {
    let calls = this.sessions.get(session);
    if (!calls) {
      calls = new Map();
      this.sessions.set(session, calls);
    }
    const snapshot = { session, call, input: structuredClone(input), cwd };
    calls.set(call, snapshot);
    return snapshot;
  }

  isCurrent(snapshot: CallSnapshot<Input>): boolean {
    return this.sessions.get(snapshot.session)?.get(snapshot.call) === snapshot;
  }

  take(session: string, call: string): CallFacts<Input> | undefined {
    const calls = this.sessions.get(session);
    const snapshot = calls?.get(call);
    calls?.delete(call);
    if (calls?.size === 0) this.sessions.delete(session);
    // A late result must not confuse an invalidated invocation with a missing before hook.
    return snapshot === null ? { selection: "off" } : snapshot;
  }

  invalidate(session?: string): void {
    const sessions = session === undefined ? this.sessions.values() : [this.sessions.get(session)];
    for (const calls of sessions) {
      if (!calls) continue;
      // Retain only IDs until their late results arrive, never input, cwd or policy.
      for (const call of calls.keys()) calls.set(call, null);
    }
  }

  clear(): void {
    this.sessions.clear();
  }
}
