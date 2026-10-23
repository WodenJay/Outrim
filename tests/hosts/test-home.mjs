import { existsSync, readFileSync } from "node:fs";
import { rm } from "node:fs/promises";
import { join } from "node:path";

export async function removeTestHome(home) {
  const endpoint = join(home, ".outrim/bookkeeping.json");
  if (existsSync(endpoint)) {
    // This resident belongs to the isolated test home, not the user's host.
    const { pid } = JSON.parse(readFileSync(endpoint, "utf8"));
    try {
      process.kill(pid);
      const deadline = Date.now() + 5000;
      while (Date.now() < deadline) {
        await new Promise((resolve) => setTimeout(resolve, 25));
        process.kill(pid, 0);
      }
      throw new Error(`Test resident ${pid} did not exit`);
    } catch (error) {
      if (error.code !== "ESRCH") throw error;
    }
  }
  // Windows may release the resident's private executable after process exit.
  await rm(home, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
}
