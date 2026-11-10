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
  "agent.runtime.unknown": "未知事件"
};

/**
 * The runtime's own event name.
 *
 * The adapter always keeps the original name in `payload.hermesEvent`, so an event the contract does
 * not model is still auditable: the UI shows the runtime's name instead of a bare "unknown".
 */
export function runtimeEventName(data: unknown): string | undefined {
  if (!data || typeof data !== "object") return undefined;
  const value = (data as Record<string, unknown>)["hermesEvent"];
  return typeof value === "string" && value.trim() !== "" ? value : undefined;
}

export function eventLabel(type: string, data?: unknown): string {
  const label = EVENT_LABELS[type] ?? type;
  if (type !== "agent.runtime.unknown") return label;
  const runtimeName = runtimeEventName(data);
  return runtimeName ? `${label} · ${runtimeName}` : label;
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
  for (const key of ["text", "message", "tool", "operation", "toolName", "reason", "summary", "status"]) {
    const value = record[key];
    if (typeof value === "string" && value.trim() !== "") return value;
  }
  const keys = Object.keys(record);
  return keys.length > 0 ? keys.slice(0, 4).join(" · ") : "";
}
