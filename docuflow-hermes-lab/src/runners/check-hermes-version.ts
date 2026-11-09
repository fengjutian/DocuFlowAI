import { execFile } from "node:child_process";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { promisify } from "node:util";
import { pathToFileURL } from "node:url";

const execFileAsync = promisify(execFile);

export interface HermesVersionLock {
  repository: string;
  tag: string;
  version: string;
  commit: string;
  releasedAt: string;
}

export function verifyHermesVersion(output: string, lock: HermesVersionLock): void {
  const normalized = output.trim();
  const versionPattern = new RegExp(`(?:^|[^0-9])v?${escapeRegex(lock.version)}(?:$|[^0-9])`);
  if (!versionPattern.test(normalized)) {
    throw new Error(`Expected Hermes ${lock.tag}, received: ${normalized || "empty version output"}`);
  }
  const commitMatch = /\b[0-9a-f]{7,40}\b/i.exec(normalized);
  if (commitMatch && !lock.commit.toLowerCase().startsWith(commitMatch[0]!.toLowerCase())) {
    throw new Error(`Hermes version matched but commit ${commitMatch[0]} did not match ${lock.commit}`);
  }
}

async function main(): Promise<void> {
  const lockPath = resolve(process.env.HERMES_LOCK_PATH ?? "hermes.lock.json");
  const lock = JSON.parse(await readFile(lockPath, "utf8")) as HermesVersionLock;
  const command = process.env.HERMES_COMMAND ?? "hermes";
  let stdout: string;
  try {
    ({ stdout } = await execFileAsync(command, ["--version"], { encoding: "utf8", windowsHide: true }));
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    throw new Error(`Unable to execute ${command} --version: ${message}`, { cause: error });
  }
  verifyHermesVersion(stdout, lock);
  console.log(`Hermes version verified: ${lock.tag} (${lock.commit})`);
}

function escapeRegex(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : undefined;
if (invokedPath === import.meta.url) {
  await main().catch(error => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  });
}
