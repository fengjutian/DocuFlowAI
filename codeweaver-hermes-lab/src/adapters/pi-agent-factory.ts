import { Agent } from "@earendil-works/pi-agent-core";
import type { MutableModels } from "@earendil-works/pi-ai";
import type {
  PiAgentEvent,
  PiAgentFactory,
  PiAgentInit,
  PiAgentLike
} from "./pi-adapter.js";

/**
 * Binds the Pi adapter to the real `pi-agent-core` runtime.
 *
 * The adapter itself is structural (see `pi-adapter.ts`) so the lab compiles and tests without
 * the Pi packages; this module is the single place where the real runtime is touched.
 */
export interface PiAgentFactoryOptions {
  /** A Pi model collection with the provider already registered. */
  models: MutableModels;
  /** The model the agent runs on. */
  model: unknown;
}

export function createPiAgentFactory(options: PiAgentFactoryOptions): PiAgentFactory {
  return (init: PiAgentInit): PiAgentLike => {
    const agent = new Agent({
      initialState: {
        ...(init.systemPrompt ? { systemPrompt: init.systemPrompt } : {}),
        // The factory owns the model collection, so it also owns the fallback model: an adapter
        // that does not name a model must still run on the configured one.
        model: (init.model ?? options.model) as never,
        tools: init.tools as never[]
      },
      streamFn: options.models.streamSimple.bind(options.models) as never,
      beforeToolCall: (async (context: {
        toolCall: { name?: string; id?: string; arguments?: unknown };
        args: unknown;
      }) => init.beforeToolCall({ toolCall: context.toolCall, args: context.args })) as never
    });

    return {
      subscribe: (listener: (event: PiAgentEvent) => void) =>
        agent.subscribe(event => listener(event as unknown as PiAgentEvent)),
      prompt: (input: string) => agent.prompt(input),
      abort: () => {
        agent.abort();
      },
      waitForIdle: () => agent.waitForIdle(),
      get state() {
        return agent.state as unknown as PiAgentLike["state"];
      }
    };
  };
}
