import { PiAdapter, type PiAdapterOptions } from "../adapters/pi-adapter.js";
import type { AgentRuntimeAdapter, AgentTaskInput } from "../types/adapter.js";

/**
 * Builds the Pi runtime adapter, if the server is configured for it.
 *
 * The Pi packages are ESM-only and are loaded dynamically: a server without Pi configured must
 * still start, and a missing package must be a clear error at boot rather than a crash on the
 * first request. The adapter factory is imported dynamically for the same reason — it is the
 * module that touches `pi-agent-core`.
 */
export interface PiRuntimeConfig {
  /** Pi provider id, e.g. `minimax-cn`. */
  provider: string;
  /** Model id within that provider, e.g. `MiniMax-M3`. */
  model: string;
  /** Builds the tool set for one execution; the tool set belongs to the execution, not the adapter. */
  tools?: (input: AgentTaskInput, executionId: string) => unknown[];
  /** Tools whose own implementation is the policy enforcement point. */
  selfEnforcedTools?: string[];
  /** Decides the tool calls DocuFlow does not own. */
  permissionHandler?: PiAdapterOptions["permissionHandler"];
}

export interface PiRuntimeResult {
  adapter: AgentRuntimeAdapter;
  modelId: string;
}

/**
 * Creates the Pi adapter for the configured provider and model.
 *
 * Only the providers this lab has evidence for are wired. A provider without a verified model
 * catalogue would fail at the first request, which is exactly the kind of late failure the Phase 0
 * rules forbid.
 */
export async function createPiRuntime(config: PiRuntimeConfig): Promise<PiRuntimeResult> {
  if (config.provider !== "minimax-cn") {
    throw new Error(`Unsupported Pi provider '${config.provider}'; this lab has evidence for minimax-cn only`);
  }

  const { createModels } = await import("@earendil-works/pi-ai");
  const { minimaxCnProvider } = await import("@earendil-works/pi-ai/providers/minimax-cn");
  const { createPiAgentFactory } = await import("../adapters/pi-agent-factory.js");

  const models = createModels();
  models.setProvider(minimaxCnProvider());
  const model = models.getModel(config.provider, config.model);
  if (!model) {
    throw new Error(`Unknown model '${config.model}' for Pi provider '${config.provider}'`);
  }

  const adapter = new PiAdapter(createPiAgentFactory({ models, model }), {
    model,
    ...(config.tools ? { tools: config.tools } : {}),
    ...(config.selfEnforcedTools ? { selfEnforcedTools: config.selfEnforcedTools } : {}),
    ...(config.permissionHandler ? { permissionHandler: config.permissionHandler } : {})
  });
  return { adapter, modelId: config.model };
}
