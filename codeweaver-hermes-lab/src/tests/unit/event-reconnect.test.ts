import test from "node:test";
import assert from "node:assert/strict";
import { AgentRuntimeRegistry } from "../../adapters/runtime-registry.js";
import { createApiServer, type ApiDependencies } from "../../api/app.js";
import { EventBus, journalEntryToEvent, type CodeWeaverEvent } from "../../api/event-bus.js";
import { InMemoryJournalStore, type JournalEntry, type JournalStore } from "../../journal/journal.js";
import type { ExecutionCoordinator } from "../../orchestrator/execution-coordinator.js";
import type { TaskExecutionService } from "../../tasks/task-execution-service.js";

/**
 * What a client sees when it loses its connection and comes back (P0-019).
 *
 * Design 13 says the socket is a notification channel and the server persists the events, so a
 * reconnect replays from `since`. That only holds if three things are true at once:
 *
 *   1. the replay and the live frames speak the same sequence space, otherwise `since` is a number
 *      the client cannot compare with what it was given;
 *   2. nothing is lost in the window between reading the journal and subscribing to the live stream;
 *   3. nothing is delivered twice, because the same event must not appear twice in a timeline.
 *
 * The second one is the trap: the old order (read the journal, then subscribe) loses every event
 * that is published in between. The test that covers it is the one that has to fail on the old
 * implementation.
 */

const EXECUTION_ID = "exec-reconnect";

interface Harness {
  url: string;
  bus: EventBus;
  journal: JournalStore;
  /** Records an event the way the coordinator does: journal first, then the live stream. */
  record(entry: JournalEntry): void;
  close(): Promise<void>;
}

/**
 * A journal whose read can be held open.
 *
 * The snapshot is taken when the read starts, so an entry appended while the read is pending is not
 * part of that read — exactly the situation a client reconnects into.
 */
class ControllableJournalStore extends InMemoryJournalStore {
  private hold: { opened: () => void; gate: Promise<void> } | undefined;

  holdNextRead(): { opened: Promise<void>; release: () => void } {
    let open!: () => void;
    let release!: () => void;
    const opened = new Promise<void>(resolve => {
      open = resolve;
    });
    const gate = new Promise<void>(resolve => {
      release = resolve;
    });
    this.hold = { opened: open, gate };
    return { opened, release: () => release() };
  }

  override async read(executionId: string): Promise<JournalEntry[]> {
    const snapshot = await super.read(executionId);
    const hold = this.hold;
    if (!hold) return snapshot;
    this.hold = undefined;
    hold.opened();
    await hold.gate;
    return snapshot;
  }
}

async function harness(options: { journal?: JournalStore } = {}): Promise<Harness> {
  const journal = options.journal ?? new InMemoryJournalStore();
  const bus = new EventBus();
  const registry = new AgentRuntimeRegistry();
  const coordinator = {
    isLive: () => false,
    getCheckpoint: async (executionId: string) => ({
      executionId,
      lastSequence: 0,
      eventCount: 0,
      terminalEventType: undefined,
      gaps: []
    })
  } as unknown as ExecutionCoordinator;
  const taskService = {} as unknown as TaskExecutionService;
  const deps: ApiDependencies = { registry, coordinator, taskService, journal, bus };
  const server = await createApiServer(deps).listen(0);

  return {
    url: server.url,
    bus,
    journal,
    record(entry: JournalEntry) {
      void journal.append(entry);
      bus.publish(journalEntryToEvent(entry));
    },
    close: () => server.close()
  };
}

function entry(sequence: number, category = "message.delta"): JournalEntry {
  return {
    executionId: EXECUTION_ID,
    sequence,
    kind: "runtime.event",
    category,
    recordedAt: new Date(Date.UTC(2026, 0, 1, 0, 0, sequence)).toISOString(),
    payload: { projectId: "proj-1", taskId: "task-1", text: `e${sequence}` }
  };
}

/**
 * Opens the stream and collects frames until `until` is satisfied.
 *
 * The collection stops on its own so a test cannot hang forever on a stream that never delivers.
 */
async function collect(
  url: string,
  options: { since?: number; until: (frames: CodeWeaverEvent[]) => boolean; timeoutMs?: number }
): Promise<CodeWeaverEvent[]> {
  const controller = new AbortController();
  const frames: CodeWeaverEvent[] = [];
  try {
    const query = options.since ? `?executionId=${EXECUTION_ID}&since=${options.since}` : `?executionId=${EXECUTION_ID}`;
    const response = await fetch(`${url}/api/v1/events${query}`, { signal: controller.signal });
    const reader = response.body!.getReader();
    const decoder = new TextDecoder();
    let buffer = "";
    const deadline = Date.now() + (options.timeoutMs ?? 2_000);
    while (!options.until(frames) && Date.now() < deadline) {
      const chunk = await reader.read();
      if (chunk.done) break;
      buffer += decoder.decode(chunk.value, { stream: true });
      let boundary = buffer.indexOf("\n\n");
      while (boundary !== -1) {
        const frame = buffer.slice(0, boundary);
        buffer = buffer.slice(boundary + 2);
        const data = frame
          .split("\n")
          .find(line => line.startsWith("data:"))
          ?.slice(5)
          .trim();
        if (data) frames.push(JSON.parse(data) as CodeWeaverEvent);
        boundary = buffer.indexOf("\n\n");
      }
    }
    return frames;
  } finally {
    controller.abort();
  }
}

test("a reconnect replays from since and the client ends up with every event exactly once", async () => {
  const app = await harness();
  try {
    app.record(entry(1));
    app.record(entry(2));
    app.record(entry(3));

    const before = await collect(app.url, { since: 0, until: frames => frames.length >= 3 });
    assert.deepEqual(before.map(frame => frame.sequence), [1, 2, 3], "the replay carries the journal");

    // The client is gone when this lands, and comes back asking for what it has not seen.
    app.record(entry(4));
    const after = await collect(app.url, { since: 3, until: frames => frames.length >= 1 });
    assert.deepEqual(after.map(frame => frame.sequence), [4], "the replay resumes at the sequence asked for");

    // What the client holds after the reconnect: the union of both, each event once.
    const seen = new Map<string, CodeWeaverEvent>();
    for (const frame of [...before, ...after]) seen.set(frame.id, frame);
    assert.deepEqual([...seen.values()].map(frame => frame.sequence).sort((a, b) => a - b), [1, 2, 3, 4]);
  } finally {
    await app.close();
  }
});

test("an event that arrives while the journal is being replayed is not lost", async () => {
  // The trap: reading the journal first and subscribing afterwards loses every event that is
  // published in between — it is in neither the replay nor the live stream.
  const journal = new ControllableJournalStore();
  const app = await harness({ journal });
  try {
    app.record(entry(1));

    const hold = journal.holdNextRead();
    const collecting = collect(app.url, { since: 0, until: frames => frames.length >= 2 });
    await hold.opened;

    // The event lands exactly in the window between the read and the subscription.
    app.record(entry(2));
    hold.release();

    const frames = await collecting;
    assert.deepEqual(frames.map(frame => frame.sequence), [1, 2], "nothing was lost in the window");
  } finally {
    await app.close();
  }
});

test("an event that arrives during the replay is not delivered twice", async () => {
  const journal = new ControllableJournalStore();
  const app = await harness({ journal });
  try {
    app.record(entry(1));

    const hold = journal.holdNextRead();
    const collecting = collect(app.url, { since: 0, until: frames => frames.length >= 2 });
    await hold.opened;

    app.record(entry(2));
    // The read is released after the write, so the replay carries the event as well as the live
    // stream does. The client must still see it once.
    await new Promise(resolve => setTimeout(resolve, 20));
    hold.release();

    const frames = await collecting;
    assert.deepEqual(frames.map(frame => frame.sequence), [1, 2], "the event is delivered exactly once");
    assert.equal(new Set(frames.map(frame => frame.id)).size, frames.length, "no frame is repeated");
  } finally {
    await app.close();
  }
});

test("the live frames carry the journal sequence, so since means the same thing on both sides", async () => {
  const app = await harness();
  try {
    app.record(entry(1, "execution.started"));
    const frames = await collect(app.url, { since: 0, until: collected => collected.length >= 1 });
    const frame = frames[0]!;

    assert.equal(frame.sequence, 1, "the live frame is numbered like the journal entry");
    assert.equal(frame.id, `${EXECUTION_ID}:1`, "the id names the execution and the journal position");
    assert.equal(frame.taskId, "task-1", "the frame carries the task it belongs to");
    assert.equal(frame.projectId, "proj-1");
  } finally {
    await app.close();
  }
});

test("lists the executions the journal knows about", async () => {
  const app = await harness();
  try {
    app.record(entry(1, "execution.started"));
    await app.journal.append({
      executionId: EXECUTION_ID,
      sequence: 2,
      kind: "audit",
      category: "execution.input",
      recordedAt: new Date().toISOString(),
      payload: { taskId: "task-1", projectId: "proj-1" }
    });
    app.record(entry(3, "execution.completed"));

    const response = await fetch(`${app.url}/api/v1/executions`);
    const body = (await response.json()) as {
      executions: Array<{
        executionId: string;
        taskId?: string;
        status: string;
        eventCount: number;
      }>;
    };

    assert.equal(response.status, 200);
    assert.equal(body.executions.length, 1);
    assert.equal(body.executions[0]!.executionId, EXECUTION_ID);
    assert.equal(body.executions[0]!.taskId, "task-1", "the task comes from the journal, not from a live event");
    assert.equal(body.executions[0]!.status, "succeeded");
    assert.equal(body.executions[0]!.eventCount, 3);
  } finally {
    await app.close();
  }
});
