import { spawnSync } from "node:child_process";
import { connect } from "node:net";
import { closeSync, openSync, readFileSync, rmSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

/**
 * The pinned Hermes gateway, as a probe resource.
 *
 * Two probes need it — the crash probe and the accounting probe — and each of them has to leave the
 * machine the way it found it. So the lifecycle lives here once: a gateway that was running is
 * running again afterwards, one that was not is stopped again, and the API key comes from the
 * pinned profile rather than from the command line.
 */

/** The lab root, so a probe can find `scripts/hermes.ps1` and `hermes.lock.json`. */
const LAB_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

export interface GatewayKill {
  /** Whether the listener was alive before the kill. */
  aliveBefore: boolean;
  /** Whether it is still alive after the kill. */
  aliveAfter: boolean;
}

export interface Gateway {
  /** The port the gateway answers on. */
  port: number;
  /** The bearer token the API server demands. */
  apiKey: string;
  /** Whether the gateway was running before the probe touched it. */
  wasRunning: boolean;
  /** Kills the gateway process. */
  kill(): GatewayKill;
  /** Brings the gateway back to the state the probe found it in. */
  restore(): Promise<void>;
}

export interface PrepareGatewayOptions {
  /** The port to reach the gateway on. */
  port?: number;
  /** The environment variable that overrides the port. */
  portEnvVar?: string;
}

/**
 * Prepares the pinned gateway.
 *
 * It is the pinned installation's own gateway, started through the project's own lifecycle script,
 * and the probe puts the machine back the way it found it: a gateway that was running is started
 * again, one that was not is left stopped.
 */
export async function prepareGateway(options: PrepareGatewayOptions = {}): Promise<Gateway> {
  const lock = JSON.parse(await readFile(join(LAB_ROOT, "hermes.lock.json"), "utf8")) as { version: string };
  const profileDir = join(process.env.LOCALAPPDATA ?? "", "hermes", `hermes-agent-${lock.version}`, "profile");
  const port = options.port ?? Number(process.env[options.portEnvVar ?? "HERMES_GATEWAY_PORT"] ?? 8642);
  const env = await readProfileEnv(profileDir);
  const apiKey = env["API_SERVER_KEY"] ?? "";
  if (apiKey === "") {
    throw new Error("the Hermes profile holds no API_SERVER_KEY, so the gateway cannot be reached");
  }

  const wasRunning = await portIsOpen(port);
  if (!wasRunning) startGateway(port);

  return {
    port,
    apiKey,
    wasRunning,
    kill: () => {
      const pid = listenerPid(port);
      const aliveBefore = pid !== undefined;
      if (pid !== undefined) {
        spawnSync("taskkill", ["/PID", String(pid), "/T", "/F"], { encoding: "utf8", timeout: 30_000 });
      }
      // Give the operating system a moment to close the socket before it is looked at again.
      const deadline = Date.now() + 10_000;
      let aliveAfter = aliveBefore;
      while (Date.now() < deadline && aliveAfter) {
        aliveAfter = listenerPid(port) !== undefined;
        if (aliveAfter) sleepQuietly(300);
      }
      return { aliveBefore, aliveAfter };
    },
    restore: async () => {
      if (wasRunning) {
        // The probe took it down; the machine had it running, so it has to be running again.
        if (!(await portIsOpen(port))) startGateway(port);
        return;
      }
      // The probe started it, so the probe stops it: the machine is left the way it was found.
      if (await portIsOpen(port)) stopGateway(port);
    }
  };
}

/** Whether something is listening on the port. */
export function portIsOpen(port: number, timeoutMs = 1500): Promise<boolean> {
  return new Promise(resolveOpen => {
    const socket = connect({ host: "127.0.0.1", port });
    const finish = (open: boolean) => {
      socket.destroy();
      resolveOpen(open);
    };
    socket.setTimeout(timeoutMs);
    socket.once("connect", () => finish(true));
    socket.once("timeout", () => finish(false));
    socket.once("error", () => finish(false));
  });
}

/** The process that owns the listening socket, if any. */
export function listenerPid(port: number): number | undefined {
  const result = spawnSync(
    "powershell",
    [
      "-NoProfile",
      "-Command",
      `(Get-NetTCPConnection -LocalPort ${port} -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1).OwningProcess`
    ],
    { encoding: "utf8", timeout: 30_000 }
  );
  const text = (result.stdout ?? "").trim();
  const pid = Number(text);
  return Number.isInteger(pid) && pid > 0 ? pid : undefined;
}

/** Starts the gateway through the project's lifecycle script. */
export function startGateway(port: number): void {
  runHermesScript("start", port);
}

/** Stops the gateway through the project's lifecycle script. */
export function stopGateway(port: number): void {
  runHermesScript("stop", port);
  // The script only stops a gateway it started itself. If the port is still answering afterwards,
  // the listener is somebody else's process and the probe leaves it alone.
  const pid = listenerPid(port);
  if (pid !== undefined) {
    spawnSync("taskkill", ["/PID", String(pid), "/T", "/F"], { encoding: "utf8", timeout: 30_000 });
  }
}

function runHermesScript(action: "start" | "stop", port: number): void {
  const script = join(LAB_ROOT, "scripts", "hermes.ps1");
  // The output goes to a file, never to a pipe: the gateway is a background process that inherits
  // the handle it was started with, and a pipe would stay open for as long as the gateway lives —
  // so the caller would wait for the gateway to exit before it could crash it.
  const logPath = join(tmpdir(), `codeweaver-hermes-${action}-${process.pid}.log`);
  const handle = openSync(logPath, "a");
  let status: number | null = null;
  let output = "";
  try {
    const result = spawnSync(
      "powershell",
      ["-ExecutionPolicy", "Bypass", "-NoProfile", "-File", script, action, "-Port", String(port)],
      { stdio: ["ignore", handle, handle], timeout: 300_000 }
    );
    status = result.status;
  } finally {
    closeSync(handle);
  }
  try {
    output = readFileSync(logPath, "utf8").trim();
  } catch {
    output = "";
  } finally {
    rmSync(logPath, { force: true });
  }
  if (action === "start" && status !== 0) {
    throw new Error(`the pinned Hermes gateway did not start on port ${port}: ${output}`);
  }
}

async function readProfileEnv(profileDir: string): Promise<Record<string, string>> {
  const values: Record<string, string> = {};
  const text = await readFile(join(profileDir, ".env"), "utf8");
  for (const line of text.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (trimmed === "" || trimmed.startsWith("#") || !trimmed.includes("=")) continue;
    const name = trimmed.slice(0, trimmed.indexOf("=")).trim();
    values[name] = trimmed.slice(trimmed.indexOf("=") + 1).trim();
  }
  return values;
}

function sleepQuietly(ms: number): void {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    // A synchronous wait: the kill path is already inside a synchronous helper.
  }
}
