/** Small display helpers shared by the pages. */

/** Formats a token count with thousands separators and a compact form for large numbers. */
export function formatTokens(value: number): string {
  if (!Number.isFinite(value)) return "-";
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(2)}M`;
  if (value >= 10_000) return `${(value / 1000).toFixed(1)}k`;
  return value.toLocaleString("en-US");
}

export function formatDuration(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return "-";
  if (ms < 1000) return `${Math.round(ms)}ms`;
  const seconds = ms / 1000;
  if (seconds < 60) return `${seconds.toFixed(1)}s`;
  const minutes = Math.floor(seconds / 60);
  const rest = Math.round(seconds % 60);
  if (minutes < 60) return `${minutes}m ${rest}s`;
  const hours = Math.floor(minutes / 60);
  return `${hours}h ${minutes % 60}m`;
}

export function formatTime(iso: string | undefined): string {
  if (!iso) return "-";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return date.toLocaleString("zh-CN", { hour12: false });
}

export function formatClock(iso: string | undefined): string {
  if (!iso) return "-";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return date.toLocaleTimeString("zh-CN", { hour12: false });
}

export function shortId(value: string | undefined, length = 8): string {
  if (!value) return "-";
  return value.length <= length ? value : `${value.slice(0, length)}…`;
}

export function relativeTime(iso: string | undefined): string {
  if (!iso) return "-";
  const time = Date.parse(iso);
  if (Number.isNaN(time)) return iso;
  const delta = Date.now() - time;
  if (delta < 0) return "刚刚";
  if (delta < 60_000) return `${Math.floor(delta / 1000)} 秒前`;
  if (delta < 3_600_000) return `${Math.floor(delta / 60_000)} 分钟前`;
  if (delta < 86_400_000) return `${Math.floor(delta / 3_600_000)} 小时前`;
  return `${Math.floor(delta / 86_400_000)} 天前`;
}

const STATUS_LABELS: Record<string, string> = {
  queued: "排队中",
  starting: "启动中",
  running: "执行中",
  waiting_approval: "等待审批",
  cancelling: "取消中",
  succeeded: "已成功",
  failed: "已失败",
  cancelled: "已取消",
  interrupted: "已中断",
  completed: "已完成",
  healthy: "健康",
  disabled: "已禁用",
  passed: "通过",
  blocked: "受阻",
  unknown: "未知"
};

export function statusLabel(status: string | undefined): string {
  if (!status) return "未知";
  return STATUS_LABELS[status] ?? status;
}

export function statusTone(status: string | undefined): "success" | "warning" | "danger" | "neutral" | "info" {
  switch (status) {
    case "succeeded":
    case "completed":
    case "passed":
    case "healthy":
      return "success";
    case "running":
    case "starting":
      return "info";
    case "waiting_approval":
    case "queued":
    case "cancelling":
    case "blocked":
      return "warning";
    case "failed":
    case "cancelled":
      return "danger";
    default:
      return "neutral";
  }
}

const EVENT_LABELS: Record<string, string> = {
  "agent.execution.started": "执行开始",
  "agent.message.delta": "输出增量",
  "agent.message.completed": "输出完成",
  "agent.tool.started": "工具开始",
  "agent.tool.progress": "工具进度",
  "agent.tool.completed": "工具完成",
  "agent.tool.failed": "工具失败",
  "agent.permission.requested": "请求权限",
  "agent.execution.waiting": "等待审批",
  "agent.execution.completed": "执行完成",
  "agent.execution.failed": "执行失败",
  "agent.execution.cancelled": "执行取消",
  "agent.runtime.unknown": "未知事件",
  // CodeWeaver's own journaled facts (audit entries), named by their category.
  "codeweaver.execution.runtime": "运行时",
  "codeweaver.execution.input": "执行请求",
  "codeweaver.execution.cancel_requested": "已请求取消",
  "codeweaver.execution.terminate_requested": "已请求终止",
  "codeweaver.context.snapshot": "上下文快照",
  "codeweaver.policy.snapshot": "策略快照",
  "codeweaver.prompt.snapshot": "提示词快照",
  "codeweaver.change.applied": "变更已应用",
  "codeweaver.permission.requested": "请求权限",
  "codeweaver.permission.resolved": "权限已裁决",
  "codeweaver.recovery.compensated": "已补偿",
  "codeweaver.recovery.rebuild": "已重建"
};

/**
 * The runtime's own event name.
 *
 * Hermes keeps it in `payload.hermesEvent`, Pi in `payload.runtimeEvent`. An event the contract does
 * not model is still auditable: the UI shows the runtime's name instead of a bare "unknown".
 */
export function runtimeEventName(data: unknown): string | undefined {
  if (!data || typeof data !== "object") return undefined;
  const record = data as Record<string, unknown>;
  for (const key of ["hermesEvent", "runtimeEvent"]) {
    const value = record[key];
    if (typeof value === "string" && value.trim() !== "") return value;
  }
  return undefined;
}

/**
 * Runtime event names that have no CodeWeaver counterpart, given a readable Chinese name.
 *
 * Pi emits a lifecycle event per streaming step (`message_update` fires for every text chunk and
 * every thinking chunk), so a single run can produce well over a hundred of them. Naming them keeps
 * the timeline readable instead of a wall of "未知事件"; the raw name still shows next to the label.
 */
const RUNTIME_EVENT_LABELS: Record<string, string> = {
  turn_start: "轮次开始",
  turn_end: "轮次结束",
  message_start: "消息开始",
  message_end: "消息结束",
  message_update: "消息更新",
  tool_execution_start: "工具开始",
  tool_execution_update: "工具更新",
  tool_execution_end: "工具结束"
};

export function eventLabel(type: string, data?: unknown): string {
  const label = EVENT_LABELS[type] ?? type;
  if (type !== "agent.runtime.unknown") return label;
  const runtimeName = runtimeEventName(data);
  if (!runtimeName) return label;
  return RUNTIME_EVENT_LABELS[runtimeName] ?? label;
}

/** Joins the text parts of an Anthropic-style content array; tool calls carry no text. */
function textFromContent(content: unknown): string | undefined {
  if (typeof content === "string") return content.trim() === "" ? undefined : content;
  if (!Array.isArray(content)) return undefined;
  const parts = content
    .map(part => {
      if (typeof part === "string") return part;
      if (part && typeof part === "object") {
        const record = part as Record<string, unknown>;
        if (record["type"] === "text" && typeof record["text"] === "string") return record["text"];
      }
      return "";
    })
    .filter(part => part !== "");
  return parts.length > 0 ? parts.join("\n") : undefined;
}

/**
 * The assistant's own words, when the payload carries them.
 *
 * Pi sends a completed message as `{ message: { content: [{ type: "text", text }] } }` and a stream
 * chunk as `{ delta }`; Hermes passes its payload through unchanged, and its terminal event carries
 * the whole answer as `output`. Without this the console only ever showed event names, and the
 * answer the user actually asked for was one "原始数据" click away — a Hermes run whose answer
 * lived in `output` looked like a run that had produced nothing at all.
 */
export function agentMessageText(data: unknown): string | undefined {
  if (!data || typeof data !== "object") return undefined;
  const record = data as Record<string, unknown>;
  for (const key of ["text", "delta", "output"]) {
    const value = record[key];
    if (typeof value === "string" && value.trim() !== "") return value;
  }
  const message = record["message"];
  if (typeof message === "string") return message.trim() === "" ? undefined : message;
  if (message && typeof message === "object") {
    const messageRecord = message as Record<string, unknown>;
    const fromContent = textFromContent(messageRecord["content"]);
    if (fromContent) return fromContent;
    const text = messageRecord["text"];
    if (typeof text === "string" && text.trim() !== "") return text;
  }
  const fromContent = textFromContent(record["content"]);
  if (fromContent) return fromContent;
  const messages = record["messages"];
  if (Array.isArray(messages)) {
    for (let index = messages.length - 1; index >= 0; index -= 1) {
      const entry = messages[index];
      if (!entry || typeof entry !== "object") continue;
      const entryRecord = entry as Record<string, unknown>;
      if (entryRecord["role"] !== undefined && entryRecord["role"] !== "assistant") continue;
      const text = textFromContent(entryRecord["content"]) ?? entryRecord["text"];
      if (typeof text === "string" && text.trim() !== "") return text;
    }
  }
  return undefined;
}

/** What a tool call returned, so the timeline can show why a step failed. */
export function toolResultText(data: unknown): string | undefined {
  if (!data || typeof data !== "object") return undefined;
  const record = data as Record<string, unknown>;
  for (const key of ["result", "partialResult", "error"]) {
    const value = record[key];
    if (typeof value === "string") return value.trim() === "" ? undefined : value;
    if (!value || typeof value !== "object") continue;
    const valueRecord = value as Record<string, unknown>;
    const fromContent = textFromContent(valueRecord["content"]);
    if (fromContent) return fromContent;
    for (const textKey of ["text", "error"]) {
      const text = valueRecord[textKey];
      if (typeof text === "string" && text.trim() !== "") return text;
    }
  }
  return undefined;
}

/** The tool a call was made against, so a result line says which step produced it. */
export function toolNameOf(data: unknown): string | undefined {
  if (!data || typeof data !== "object") return undefined;
  const value = (data as Record<string, unknown>)["toolName"];
  return typeof value === "string" && value.trim() !== "" ? value : undefined;
}

/** Renders an unknown payload as readable JSON, capped so one huge delta cannot break the timeline. */
export function previewPayload(data: unknown, maxLength = 600): string {
  if (data === undefined || data === null) return "";
  if (typeof data === "string") return data.length > maxLength ? `${data.slice(0, maxLength)}…` : data;
  let text: string;
  try {
    text = JSON.stringify(data, null, 2) ?? String(data);
  } catch {
    text = String(data);
  }
  return text.length > maxLength ? `${text.slice(0, maxLength)}…` : text;
}

/** Pulls the most useful one-line summary out of a runtime payload. */
export function summarizePayload(data: unknown): string {
  if (!data || typeof data !== "object") return typeof data === "string" ? data : "";
  const record = data as Record<string, unknown>;
  // What the agent said, or what the tool returned, beats a list of payload keys.
  const message = agentMessageText(data);
  if (message) return message;
  const toolResult = toolResultText(data);
  if (toolResult) return toolResult;
  for (const key of ["tool", "operation", "toolName", "reason", "summary", "status", "runtimeId", "taskId"]) {
    const value = record[key];
    if (typeof value === "string" && value.trim() !== "") return value;
  }
  const keys = Object.keys(record);
  return keys.length > 0 ? keys.slice(0, 4).join(" · ") : "";
}
