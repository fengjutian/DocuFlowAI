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

/**
 * Release tarballs of the pinned Hermes ship `version = "0.0.0"` in pyproject.toml; the real
 * identity lives in `install-stamp.json` at the install root (upstream's own resolution order:
 * stamp -> live git -> unknown). Reporting 0.0.0 means the stamp was not written or not found.
 */
const RELEASE_PLACEHOLDER_VERSION = "0.0.0";

/**
 * Reads the installed version and, when the install carries one, the release commit.
 *
 * The pinned release ships `version = "0.0.0"` in pyproject.toml, so package metadata alone
 * cannot answer "which Hermes is this?". Upstream keeps the real identity in
 * `install-stamp.json` at the install root, and resolves it before git. This snippet mirrors
 * that order: stamp first, package metadata as the fallback for a plain pip install.
 *
 * Prints `<version> <commit>`; the commit is empty when no stamp was found.
 */
const VERSION_SNIPPET = [
  "import json, os, pathlib, sys",
  "version = ''",
  "commit = ''",
  "try:",
  "    import importlib.metadata as metadata",
  "    version = metadata.version('hermes-agent')",
  "except Exception:",
  "    pass",
  "root = os.environ.get('HERMES_INSTALL_DIR', '').strip()",
  "if not root:",
  "    executable = pathlib.Path(sys.executable).resolve()",
  "    root = str(executable.parent.parent.parent) if executable.parent.name.lower() == 'scripts' else str(executable.parent)",
  "stamp = pathlib.Path(root) / 'install-stamp.json'",
  "if stamp.is_file():",
  "    try:",
  "        data = json.loads(stamp.read_text(encoding='utf-8-sig'))",
  "        version = data.get('baseVersion') or version",
  "        commit = data.get('commit') or ''",
  "    except Exception:",
  "        pass",
  "print(version, commit)"
].join("\n");

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
  if (reported === RELEASE_PLACEHOLDER_VERSION) {
    throw new Error(
      `Hermes reported ${RELEASE_PLACEHOLDER_VERSION}, which is the placeholder the release ` +
        `tarball ships in pyproject.toml, not a version. The pinned runtime states its identity in ` +
        `install-stamp.json at the install root: write it with ` +
        `scripts/write_install_stamp.py --base-version ${lock.version} --commit ${lock.commit} ` +
        `--update-mechanism self, or point HERMES_INSTALL_DIR at that root.`
    );
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
      ({ stdout } = await execFileAsync(python, ["-c", VERSION_SNIPPET], {
        encoding: "utf8",
        windowsHide: true
      }));
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
