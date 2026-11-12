import { useState } from "react";
import { History, Loader2, Search } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Separator } from "@/components/ui/separator";
import { EmptyState, ErrorNotice } from "@/components/common/EmptyState";
import { Badge } from "@/components/ui/badge";
import { api } from "@/lib/api";
import { formatClock, formatTime, shortId } from "@/lib/format";
import type { AuditTimeline, AuditTimelineItem } from "@/lib/types";

/**
 * The audit timeline (design 15: "从审计事件还原完整执行过程").
 *
 * The trace page answers "which records belong together"; this one answers "what happened, and in
 * what order". Everything on it comes from the server's journal — the same append-only record the
 * live stream replays — so the console never rebuilds the history from what its own socket happened
 * to receive.
 *
 * Two orders live here and the page shows both: the events are listed by the recorded time across
 * executions, and each execution keeps its own journal order inside it. An execution whose
 * timestamps disagree with its own order is called out rather than smoothed away.
 */

type Scope = "all" | "task" | "execution";

const CATEGORY_LABELS: Record<string, string> = {
  "execution.input": "执行输入",
  "execution.runtime": "运行时",
  "execution.started": "运行开始",
  "execution.completed": "执行完成",
  "execution.failed": "执行失败",
  "execution.cancelled": "执行取消",
  "execution.cancel_requested": "请求取消",
  "execution.terminate_requested": "请求终止",
  "turn.started": "轮次开始",
  "tool.started": "工具开始",
  "tool.completed": "工具完成",
  "tool.failed": "工具失败",
  "permission.requested": "权限请求",
  "permission.resolved": "权限裁决",
  "change.applied": "变更已应用",
  "evaluation.completed": "验收完成",
  "context.snapshot": "上下文快照",
  "context.snapshot.skipped": "跳过上下文快照",
  "policy.snapshot": "策略快照",
  "prompt.snapshot": "提示词快照",
  "skill.bound": "技能绑定",
  "skill.delivered": "技能注入",
  "subagent.spawned": "派生子代理",
  "subagent.parent": "父执行",
  "recovery.rebuild": "重建恢复",
  "recovery.compensated": "补偿恢复"
};

const PHASE_LABELS: Record<string, string> = {
  started: "开始",
  turn: "轮次",
  policy: "策略",
  tool: "工具",
  change: "变更",
  evaluation: "验收",
  skill: "技能",
  subagent: "子代理",
  recovery: "恢复",
  completed: "完成",
  failed: "失败",
  cancelled: "取消"
};

/** The one fact of an event that a reader wants, picked per category. */
function eventDetail(item: AuditTimelineItem): string {
  const payload = item.payload ?? {};
  const text = (value: unknown): string | undefined =>
    typeof value === "string" && value !== "" ? value : undefined;
  switch (item.category) {
    case "change.applied":
      return [text(payload["gitCommitSha"])?.slice(0, 12), text(payload["changeRecordId"])]
        .filter(Boolean)
        .join(" · ");
    case "evaluation.completed":
      return [text(payload["verdict"]), payload["checks"] !== undefined ? `${String(payload["checks"])} 项检查` : undefined]
        .filter(Boolean)
        .join(" · ");
    case "tool.started":
    case "tool.completed":
    case "tool.failed":
      return text(payload["toolName"]) ?? "";
    case "permission.resolved":
      return [text(payload["decision"]), text(payload["reason"])].filter(Boolean).join(" · ");
    case "permission.requested":
      return text(payload["tool"]) ?? "";
    case "execution.runtime":
      return text(payload["runtimeId"]) ?? "";
    case "execution.input":
      return text(payload["taskId"]) ?? "";
    case "subagent.spawned":
      return text(payload["childExecutionId"]) ?? "";
    case "subagent.parent":
      return text(payload["parentExecutionId"]) ?? "";
    default:
      return "";
  }
}

export function AuditTimelinePage() {
  const [scope, setScope] = useState<Scope>("all");
  const [query, setQuery] = useState("");
  const [timeline, setTimeline] = useState<AuditTimeline | undefined>(undefined);
  const [error, setError] = useState<unknown>(undefined);
  const [loading, setLoading] = useState(false);

  const lookup = async () => {
    const value = query.trim();
    setLoading(true);
    setError(undefined);
    try {
      if (scope === "all") {
        setTimeline(await api.auditTimeline());
        return;
      }
      if (value === "") {
        setError(new Error("请填写要查询的任务或执行 ID。"));
        setTimeline(undefined);
        return;
      }
      setTimeline(
        scope === "task"
          ? await api.auditTimeline({ taskId: value })
          : await api.auditTimeline({ executionId: value })
      );
    } catch (caught) {
      setTimeline(undefined);
      setError(caught);
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="space-y-4">
      <Card className="border-border/70 bg-card/60">
        <CardHeader>
          <CardTitle className="text-base">审计时间线</CardTitle>
          <CardDescription>
            从审计事件还原完整执行过程。事件取自服务端 journal（实时流重放的就是同一份记录），按记录时间排列，每个执行内部保持自己的 journal 顺序。
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-3">
          <div className="flex flex-wrap gap-2">
            <Select value={scope} onValueChange={value => setScope(value as Scope)}>
              <SelectTrigger id="timeline-scope" className="w-44">
                <SelectValue placeholder="选择查询方式" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">全部执行</SelectItem>
                <SelectItem value="task">按任务 ID</SelectItem>
                <SelectItem value="execution">按执行 ID</SelectItem>
              </SelectContent>
            </Select>
            <Input
              value={query}
              disabled={scope === "all"}
              placeholder={scope === "execution" ? "exec_..." : "TASK-001"}
              className="font-mono"
              onChange={event => setQuery(event.target.value)}
              onKeyDown={event => {
                if (event.key === "Enter") void lookup();
              }}
            />
            <Button onClick={() => void lookup()} disabled={loading}>
              {loading ? <Loader2 className="size-4 animate-spin" /> : <Search className="size-4" />}
              查询
            </Button>
          </div>

          {error ? <ErrorNotice error={error} /> : null}

          {!timeline && !error && !loading ? (
            <EmptyState
              icon={History}
              title="还没有查询结果"
              description="时间线完全由服务端记录生成，界面不推断事件之间的关系。"
            />
          ) : null}

          {timeline ? <TimelineSummary timeline={timeline} /> : null}
        </CardContent>
      </Card>

      {timeline && timeline.executions.length > 0 ? (
        <ExecutionCards timeline={timeline} />
      ) : timeline && !error ? (
        <EmptyState
          icon={History}
          title="这段时间线里没有事件"
          description="服务端没有记录任何符合条件的执行。这个任务可能从未启动，或者它的执行在写下第一条记录之前就结束了。"
        />
      ) : null}
    </div>
  );
}

function TimelineSummary({ timeline }: { timeline: AuditTimeline }) {
  return (
    <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
      <Badge variant="outline">{timeline.executions.length} 个执行</Badge>
      <Badge variant="outline">{timeline.items.length} 条事件</Badge>
      {timeline.taskIds.length > 0 ? <Badge variant="outline">任务 {timeline.taskIds.join(", ")}</Badge> : null}
      {timeline.gaps.length > 0 ? (
        <Badge variant="outline" className="border-amber-500/40 text-amber-600 dark:text-amber-400">
          {timeline.gaps.length} 个执行有缺口
        </Badge>
      ) : null}
      {timeline.truncated ? <Badge variant="outline">已截断</Badge> : null}
    </div>
  );
}

/**
 * One card per execution, with the events of every execution interleaved by the recorded time.
 *
 * The stages are derived from the recorded events, so a run that never changed anything shows no
 * change stage — it is not filled in to look complete.
 */
function ExecutionCards({ timeline }: { timeline: AuditTimeline }) {
  const itemsByExecution = new Map<string, AuditTimelineItem[]>();
  for (const item of timeline.items) {
    const bucket = itemsByExecution.get(item.executionId) ?? [];
    bucket.push(item);
    itemsByExecution.set(item.executionId, bucket);
  }

  return (
    <div className="space-y-4">
      {timeline.executions.map(execution => {
        const items = itemsByExecution.get(execution.executionId) ?? [];
        return (
          <Card key={execution.executionId} className="border-border/70 bg-card/60">
            <CardHeader>
              <CardTitle className="flex flex-wrap items-center gap-2 text-base">
                <History className="size-4 text-muted-foreground" />
                <span className="font-mono text-sm">{execution.executionId}</span>
                {execution.runtimeId ? (
                  <Badge variant="outline" className="font-mono text-[10px]">
                    {execution.runtimeId}
                  </Badge>
                ) : null}
              </CardTitle>
              <CardDescription className="flex flex-wrap items-center gap-x-3 gap-y-1">
                <span>
                  {execution.taskIds.length > 0 ? `任务 ${execution.taskIds.join(", ")}` : "无任务记录"}
                </span>
                <span>{execution.eventCount} 条事件</span>
                <span>
                  {formatTime(execution.firstRecordedAt)} → {formatTime(execution.lastRecordedAt)}
                </span>
                {execution.parentExecutionId ? (
                  <span className="font-mono">父执行 {shortId(execution.parentExecutionId, 12)}</span>
                ) : null}
                {execution.childExecutionIds.length > 0 ? (
                  <span className="font-mono">子执行 {execution.childExecutionIds.length} 个</span>
                ) : null}
              </CardDescription>
            </CardHeader>
            <CardContent className="space-y-3">
              <div className="flex flex-wrap items-center gap-1.5">
                {execution.phases.map(phase => (
                  <Badge key={phase} variant="outline" className="text-[10px]">
                    {PHASE_LABELS[phase] ?? phase}
                  </Badge>
                ))}
                {execution.gaps.length > 0 ? (
                  <Badge
                    variant="outline"
                    className="border-amber-500/40 text-[10px] text-amber-600 dark:text-amber-400"
                  >
                    缺口 {execution.gaps.join(",")}
                  </Badge>
                ) : null}
                {!execution.timestampsMonotonic ? (
                  <Badge
                    variant="outline"
                    className="border-amber-500/40 text-[10px] text-amber-600 dark:text-amber-400"
                  >
                    时间戳与记录顺序不一致
                  </Badge>
                ) : null}
              </div>

              <Separator className="bg-border/60" />

              <ol className="space-y-1.5">
                {items.map(item => (
                  <li
                    key={`${item.executionId}:${item.sequence}`}
                    className="flex flex-wrap items-center gap-x-3 gap-y-1 rounded-md border border-border/50 bg-background/30 px-2.5 py-1.5 text-xs"
                  >
                    <span className="font-mono text-muted-foreground">{formatClock(item.recordedAt)}</span>
                    <span className="font-mono text-muted-foreground">#{item.sequence}</span>
                    <span className="min-w-0 flex-1 truncate">
                      {CATEGORY_LABELS[item.category] ?? item.category}
                      {eventDetail(item) ? (
                        <span className="ml-2 font-mono text-muted-foreground">{eventDetail(item)}</span>
                      ) : null}
                    </span>
                    <span className="font-mono text-[10px] text-muted-foreground">
                      {shortId(item.executionId, 12)}
                    </span>
                  </li>
                ))}
              </ol>
            </CardContent>
          </Card>
        );
      })}
    </div>
  );
}
