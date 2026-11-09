import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { mergeAgentServer, removeAgentServer } from "../../client/zed-settings.js";
import { defaultZedSettingsPath, installZedAgent } from "../../runners/install-zed.js";

const entry = {
  command: "C:\\Program Files\\nodejs\\node.exe",
  args: ["D:/repo/dist/runners/acp-server.js"],
  env: { HERMES_HTTP_BASE_URL: "http://127.0.0.1:8642" }
};

test("registers the agent without disturbing other Zed settings", () => {
  const settings = {
    theme: "One Dark",
    vim_mode: true,
    agent_servers: { other: { command: "other-agent" } }
  };

  const { settings: merged, changed } = mergeAgentServer(settings, "docuflow", entry);

  assert.equal(changed, true);
  assert.equal(merged["theme"], "One Dark");
  assert.equal(merged["vim_mode"], true);
  assert.deepEqual((merged["agent_servers"] as Record<string, unknown>)["other"], { command: "other-agent" });
  assert.deepEqual((merged["agent_servers"] as Record<string, unknown>)["docuflow"], entry);
});

test("is idempotent so re-running the installer changes nothing", () => {
  const first = mergeAgentServer({}, "docuflow", entry);
  const second = mergeAgentServer(first.settings, "docuflow", entry);

  assert.equal(first.changed, true);
  assert.equal(second.changed, false);
  assert.deepEqual(second.settings, first.settings);
});

test("updates an outdated entry instead of duplicating it", () => {
  const first = mergeAgentServer({}, "docuflow", entry);
  const updated = mergeAgentServer(first.settings, "docuflow", { ...entry, args: ["D:/repo/other.js"] });

  assert.equal(updated.changed, true);
  assert.deepEqual(
    ((updated.settings["agent_servers"] as Record<string, unknown>)["docuflow"] as { args: string[] }).args,
    ["D:/repo/other.js"]
  );
});

test("removes only the named agent", () => {
  const settings = { agent_servers: { docuflow: entry, keep: { command: "x" } } };
  const removed = removeAgentServer(settings, "docuflow");

  assert.equal(removed.changed, true);
  assert.deepEqual(removed.settings, { agent_servers: { keep: { command: "x" } } });
  assert.equal(removeAgentServer(removed.settings, "docuflow").changed, false);
});

test("rejects empty names and commands", () => {
  assert.throws(() => mergeAgentServer({}, " ", entry), /name/);
  assert.throws(() => mergeAgentServer({}, "docuflow", { command: " " }), /command/);
});

test("installs into a real settings file and keeps a backup", async () => {
  const root = await mkdtemp(join(tmpdir(), "docuflow-zed-"));
  const settingsPath = join(root, "settings.json");
  try {
    await writeFile(settingsPath, JSON.stringify({ theme: "One Dark" }, null, 2), "utf8");

    const result = await installZedAgent({
      settingsPath,
      name: "docuflow",
      entry,
      now: () => new Date("2026-11-09T00:00:00.000Z")
    });

    assert.equal(result.changed, true);
    assert.ok(result.backupPath);
    const written = JSON.parse(await readFile(settingsPath, "utf8")) as Record<string, unknown>;
    assert.equal(written["theme"], "One Dark");
    assert.ok("docuflow" in (written["agent_servers"] as Record<string, unknown>));

    const backup = JSON.parse(await readFile(result.backupPath!, "utf8")) as Record<string, unknown>;
    assert.deepEqual(backup, { theme: "One Dark" });

    const second = await installZedAgent({ settingsPath, name: "docuflow", entry });
    assert.equal(second.changed, false);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("creates a settings file when Zed has none yet", async () => {
  const root = await mkdtemp(join(tmpdir(), "docuflow-zed-"));
  const settingsPath = join(root, "nested", "settings.json");
  try {
    const result = await installZedAgent({ settingsPath, name: "docuflow", entry });
    assert.equal(result.changed, true);
    assert.equal(result.backupPath, undefined);
    const written = JSON.parse(await readFile(settingsPath, "utf8")) as Record<string, unknown>;
    assert.deepEqual(Object.keys(written), ["agent_servers"]);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("refuses to overwrite a settings file that is not valid JSON", async () => {
  const root = await mkdtemp(join(tmpdir(), "docuflow-zed-"));
  const settingsPath = join(root, "settings.json");
  try {
    await writeFile(settingsPath, "{ // comments }", "utf8");
    await assert.rejects(installZedAgent({ settingsPath, name: "docuflow", entry }), /not valid JSON/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("resolves the platform settings path and honors an override", () => {
  assert.match(
    defaultZedSettingsPath({ ZED_SETTINGS_PATH: "C:/custom/settings.json" } as NodeJS.ProcessEnv),
    /custom[\\/]settings\.json$/
  );
  if (process.platform === "win32") {
    assert.match(
      defaultZedSettingsPath({ APPDATA: "C:/Users/x/AppData/Roaming" } as NodeJS.ProcessEnv),
      /Zed[\\/]settings\.json$/
    );
  }
});
