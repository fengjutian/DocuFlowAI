import { create } from "zustand";
import { persist } from "zustand/middleware";

/**
 * Backend connection settings.
 *
 * The lab server is not at a fixed place: `CODEWEAVER_API_PORT` and `CODEWEAVER_API_TOKEN` decide where it
 * listens and whether it demands a bearer token. The desktop app therefore asks for the address
 * instead of hard-coding it, and the Tauri side may seed a default from the environment.
 */
export const DEFAULT_API_BASE_URL = "http://127.0.0.1:8787";

export interface ConnectionSettings {
  baseUrl: string;
  token: string;
  setBaseUrl: (value: string) => void;
  setToken: (value: string) => void;
}

export const useConnection = create<ConnectionSettings>()(
  persist(
    set => ({
      baseUrl: DEFAULT_API_BASE_URL,
      token: "",
      setBaseUrl: baseUrl => set({ baseUrl: baseUrl.trim() }),
      setToken: token => set({ token: token.trim() })
    }),
    { name: "codeweaver.connection" }
  )
);

/** Normalizes a user-typed address into a base URL without a trailing slash. */
export function normalizeBaseUrl(raw: string): string {
  const trimmed = raw.trim();
  if (trimmed === "") return DEFAULT_API_BASE_URL;
  return trimmed.endsWith("/") ? trimmed.slice(0, -1) : trimmed;
}

export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

interface StoredSettings {
  baseUrl?: string;
  token?: string;
}

function readStoredSettings(): StoredSettings {
  try {
    const raw = localStorage.getItem("codeweaver.connection");
    if (!raw) return {};
    const parsed = JSON.parse(raw) as { state?: StoredSettings };
    return parsed.state ?? {};
  } catch {
    return {};
  }
}

/**
 * Seeds the connection settings from the desktop runtime.
 *
 * The Rust side can provide `CODEWEAVER_API_URL` / `CODEWEAVER_API_TOKEN`, so a launcher can point the
 * app at a server without the user retyping it. Stored values always win: an explicit choice by the
 * user is never overwritten by an environment default.
 */
export async function seedFromRuntimeDefaults(): Promise<void> {
  if (!isTauri()) return;
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    const stored = readStoredSettings();
    const baseUrl = await invoke<string | null>("default_api_base_url");
    if (baseUrl && !stored.baseUrl) useConnection.getState().setBaseUrl(baseUrl);
    const token = await invoke<string | null>("default_api_token");
    if (token && !stored.token) useConnection.getState().setToken(token);
  } catch {
    // The commands are optional: the client is fully usable with manually entered settings.
  }
}
