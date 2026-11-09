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
  const reported = extractVersion(normalized);
  if (!reported) {
    if (/Failed to initialize agent|No LLM provider configured/i.test(normalized)) {
      throw new Error(
        "Hermes started the agent instead of printing a version (it initializes a provider before " +
          "parsing flags). Set HERMES_PYTHON to the venv interpreter so the installed package " +
          "metadata can be read, or HERMES_VERSION_COMMAND to a command that prints the version."
      );
    }
    throw new Error(`Expected Hermes ${lock.tag}, received: ${normalized || "empty version output"}`);
  }
  if (reported !== lock.version) {
    throw new Error(`Installed Hermes is ${reported}, but hermes.lock.json pins ${lock.version} (${lock.tag})`);
  }
  const commitMatch = /\b[0-9a-f]{7,40}\b/i.exec(normalized);
  if (commitMatch && !lock.commit.toLowerCase().startsWith(commitMatch[0]!.toLowerCase())) {
    throw new Error(`Hermes version matched but commit ${commitMatch[0]} did not match ${lock.commit}`);
  }
}

/** First dotted version in the output; tolerates `hermes-agent 0.23.0` and bare `v0.23.0`. */
export function extractVersion(output: string): string | undefined {
  const match = /\bv?(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)\b/.exec(output);
  return match?.[1];
}

async function main(): Promise<void> {
  const lockPath = resolve(process.env.HERMES_LOCK_PATH ?? "hermes.lock.json");
  const lock = JSON.parse(await readFile(lockPath, "utf8")) as HermesVersionLock;
  const override = process.env.HERMES_VERSION_COMMAND;
  const python = process.env.HERMES_PYTHON;
  const command = process.env.HERMES_COMMAND ?? "hermes";

  let stdout: string;
  try {
    if (override) {
      ({ stdout } = await execFileAsync(override, [], { encoding: "utf8", windowsHide: true }));
    } else if (python) {
      ({ stdout } = await execFileAsync(python, [
        "-c",
        "import importlib.metadata as m; print(m.version('hermes-agent'))"
      ], { encoding: "utf8", windowsHide: true }));
    } else {
      ({ stdout } = await execFileAsync(command, ["--version"], { encoding: "utf8", windowsHide: true }));
    }
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    throw new Error(`Unable to determine the Hermes version: ${message}`, { cause: error });
  }
  verifyHermesVersion(stdout, lock);
  console.log(`Hermes version verified: ${lock.tag} (${lock.commit})`);
}


const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : undefined;
if (invokedPath === import.meta.url) {
  await main().catch(error => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  });
}
