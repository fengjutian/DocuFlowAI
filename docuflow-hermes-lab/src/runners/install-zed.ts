import { copyFile, mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { applyAgentServerToText, type AgentServerEntry } from "../client/zed-settings.js";

/**
 * Registers DocuFlow as a Zed external agent.
 *
 * Zed spawns the command from `agent_servers` and speaks ACP over stdio. The settings file is JSONC, so
 * edits preserve comments and formatting; only that one key changes, the file is backed up first, and
 * `--dry-run` reports the change without writing.
 */

export interface InstallOptions {
  settingsPath: string;
  name: string;
  entry: AgentServerEntry;
  dryRun?: boolean;
  now?: () => Date;
}

export interface InstallResult {
  settingsPath: string;
  changed: boolean;
  backupPath?: string;
}

export async function installZedAgent(options: InstallOptions): Promise<InstallResult> {
  let raw = "";
  try {
    raw = await readFile(options.settingsPath, "utf8");
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
  }

  const edited = applyAgentServerToText(raw, options.name, options.entry);
  if (!edited.changed) return { settingsPath: options.settingsPath, changed: false };
  if (options.dryRun) return { settingsPath: options.settingsPath, changed: true };

  await mkdir(dirname(options.settingsPath), { recursive: true });
  let backupPath: string | undefined;
  if (raw.trim()) {
    const stamp = (options.now ?? (() => new Date()))().toISOString().replace(/[:.]/g, "-");
    backupPath = `${options.settingsPath}.bak-${stamp}`;
    await copyFile(options.settingsPath, backupPath);
  }
  await writeFile(options.settingsPath, edited.text, "utf8");
  return { settingsPath: options.settingsPath, changed: true, ...(backupPath ? { backupPath } : {}) };
}

export function defaultZedSettingsPath(env: NodeJS.ProcessEnv = process.env): string {
  if (env["ZED_SETTINGS_PATH"]) return resolve(env["ZED_SETTINGS_PATH"]);
  if (process.platform === "win32") {
    const appData = env["APPDATA"];
    if (!appData) throw new Error("APPDATA is not set; pass ZED_SETTINGS_PATH instead");
    return resolve(appData, "Zed", "settings.json");
  }
  const home = env["HOME"];
  if (!home) throw new Error("HOME is not set; pass ZED_SETTINGS_PATH instead");
  return resolve(home, ".config", "zed", "settings.json");
}

async function main(): Promise<void> {
  const settingsPath = defaultZedSettingsPath();
  const runner = resolve(process.env["DOCUFLOW_ACP_RUNNER"] ?? "dist/runners/acp-server.js");
  const dataDir = resolve(process.env["DOCUFLOW_DATA_DIR"] ?? ".docuflow-data");
  const baseUrl = process.env["HERMES_HTTP_BASE_URL"] ?? "http://127.0.0.1:8642";

  const result = await installZedAgent({
    settingsPath,
    name: "docuflow",
    dryRun: process.argv.includes("--dry-run"),
    entry: {
      command: process.execPath,
      args: [runner],
      env: { HERMES_HTTP_BASE_URL: baseUrl, DOCUFLOW_DATA_DIR: dataDir }
    }
  });

  if (!result.changed) {
    console.log(`DocuFlow is already registered in ${result.settingsPath}`);
    return;
  }
  if (process.argv.includes("--dry-run")) {
    console.log(`Would register DocuFlow in ${result.settingsPath}`);
    return;
  }
  console.log(`Registered DocuFlow in ${result.settingsPath}`);
  if (result.backupPath) console.log(`Backup: ${result.backupPath}`);
  console.log("Restart Zed, then pick DocuFlow under the agent panel.");
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : undefined;
if (invokedPath === import.meta.url) {
  await main().catch(error => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  });
}
