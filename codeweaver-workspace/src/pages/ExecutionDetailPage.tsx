import { useEffect, useMemo, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import {
  ArrowLeft,
  CircleCheck,
  CircleX,
  Clock,
  FileDiff,
  Gauge,
  KeyRound,
  Loader2,
  MessageSquare,
  Plug,
  RefreshCw,
  RotateCcw,
  ScrollText,
  Sparkles,
  Square
} from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Separator } from "@/components/ui/separator";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { EmptyState, ErrorNotice } from "@/components/common/EmptyState";
import { StatusBadge } from "@/components/common/StatusBadge";
import { api } from "@/lib/api";
import {
  agentMessageText,
  eventLabel,
  formatClock,
  formatDuration,
  formatTime,
  formatTokens,
  previewPayload,
  runtimeEventName,
  summarizePayload,
  toolNameOf,
  toolResultText
} from "@/lib/format";
import { useApiData } from "@/hooks/useApiData";
import { useExecutionEvents } from "@/state/execution-stream";
import type { CodeWeaverEvent } from "@/lib/types";

const EVENT_TONE: Record<string, string> = {
  "agent.execution.started": "text-sky-300",
  "agent.message.completed": "text-sky-200",
  "agent.message.delta": "text-sky-200/70",
  "agent.tool.completed": "text-emerald-300",
  "agent.execution.completed": "text-emerald-300",
  "agent.execution.failed": "text-red-300",
  "agent.execution.cancelled": "text-amber-300",
  "agent.execution.waiting": "text-amber-300",
  "agent.permission.requested": "text-amber-300",
  "agent.tool.failed": "text-red-300"
};

/**
 * Execution detail.
 *
 * The page shows two independent facts side by side: what the runtime reported (the event timeline)
 * and what CodeWeaver concluded (checkpoint, completion gate, approvals). A run that emitted
 * `execution.completed` is not "done" until the completion gate says it may be.
 */
export function ExecutionDetailPage() {
  const { executionId = "" } = useParams();
  const navigate = useNavigate();
  const [busy, setBusy] = useState<string | undefined>(undefined);

  const detail = useApiData(() => api.getExecution(executionId), [executionId], { pollMs: 3000 });
  const history = useApiData(() => api.getExecutionEvents(executionId), [executionId]);
  const usage = useApiData(() => api.getExecutionUsage(executionId), [executionId]);
  const skills = useApiData(() => api.listExecutionSkills(executionId), [executionId]);
  const registry = useApiData(() => api.listSkills(), []);
  const liveEvents = useExecutionEvents(executionId);

  // The journal holds the authoritative history; the stream adds what happens after this page
  // opened. Merging by sequence keeps one ordered timeline.
  const timeline = useMemo<CodeWeaverEvent[]>(() => {
    const merged = new Map<number, CodeWeaverEvent>();
    for (const event of history.data?.events ?? []) merged.set(event.sequence, event);
    for (const event of liveEvents) merged.set(event.sequence, event);
    return [...merged.values()].sort((a, b) => a.sequence - b.sequence);
  }, [history.data, liveEvents]);

  /**
   * The assistant's last message, in full.
   *
   * A run's answer is the one thing the user came for, and the timeline only carries it inside an
   * event payload. Taking the last completed assistant message keeps the page honest about a
   * cancelled run too: it shows what the runtime produced before the abort, not a friendly summary.
   */
  const lastOutput = useMemo(() => {
    const texts: string[] = [];
    for (const event of timeline) {
      if (event.type !== "agent.message.completed") continue;
      const text = agentMessageText(event.data);
      if (text) texts.push(text);
    }
    if (texts.length === 0) {
      for (const event of timeline) {
        if (event.type !== "agent.execution.completed") continue;
        const text = agentMessageText(event.data);
        if (text) texts.push(text);
      }
    }
    return texts.length > 0 ? texts[texts.length - 1] : undefined;
  }, [timeline]);

  /**
   * The skill deliveries this run recorded.
   *
   * A binding says a skill was attached; a delivery says its body reached the runtime. Only the
   * second one is what the model actually read.
   */
  const deliveries = useMemo(() => {
    const found: Array<{ skillId: string; version: string; deliveryMode?: string }> = [];
    for (const event of timeline) {
      if (event.type !== "codeweaver.skill.delivered") continue;
      const skillId = String(event.data?.["skillId"] ?? "");
      if (!skillId) continue;
      found.push({
        skillId,
        version: String(event.data?.["version"] ?? ""),
        deliveryMode: String(event.data?.["deliveryMode"] ?? "")
      });
    }
    return found;
  }, [timeline]);

  useEffect(() => {
    if (detail.error) toast.error(detail.error.message);
    // Only report the first failure of a given execution; polling would otherwise spam.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [executionId]);

  const run = async (name: string, action: () => Promise<string | undefined>) => {
    setBusy(name);
    try {
      const message = await action();
      if (message) toast.success(message);
      detail.refresh();
      history.refresh();
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(undefined);
    }
  };

  const approvals = detail.data?.pendingApprovals ?? [];
  const status = detail.data?.status;
  // An execution recorded by an earlier server process is readable but not actionable: there is no
  // live session to cancel, resume or rebuild from, so the buttons say so instead of failing.
  const live = detail.data?.live !== false;

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex min-w-0 items-center gap-3">
          <Button variant="ghost" size="icon" onClick={() => navigate("/executions")}>
            <ArrowLeft className="size-4" />
          </Button>
          <div className="min-w-0">
            <div className="flex items-center gap-2">
              <h2 className="truncate font-mono text-sm">{executionId}</h2>
              <StatusBadge status={status} />
            </div>
            <p className="text-xs text-muted-foreground">
              {/* The runtime that produced this timeline, so a reader knows which one to trust. */}
              <span className="font-mono">{detail.data?.runtimeId ?? "未知运行时"}</span> ·{" "}
              {detail.data?.checkpoint.currentStep ?? "—"} ·{" "}
              {detail.data?.checkpoint.eventCount ?? timeline.length} 条事件
            </p>
            {live ? null : (
              <p className="text-xs text-amber-300/90">
                该执行由更早的服务进程运行：记录与时间线仍在，但已经没有活的会话可取消或恢复。
              </p>
            )}
          </div>
        </div>

        <div className="flex items-center gap-2">
          <Button
            variant="outline"
            size="sm"
            disabled={busy !== undefined || !live}
            title={live ? undefined : "该执行不在当前服务进程内运行，无法取消"}
            onClick={() =>
              void run("cancel", async () => {
                await api.cancelExecution(executionId);
                return "已请求取消";
              })
            }
          >
            {busy === "cancel" ? <Loader2 className="size-4 animate-spin" /> : <Square className="size-4" />}
            取消
          </Button>
          <Button
            variant="outline"
            size="sm"
            disabled={busy !== undefined}
            onClick={() =>
              void run("retry", async () => {
                const result = await api.retryExecution(executionId);
                // A terminal execution has nothing to recover: the plan says "stop" and no new
                // execution is created. Navigating to an undefined id would be a dead link.
                if (!result.execution?.executionId) {
                  return `无需重试：${result.plan.reason}`;
                }
                navigate(`/executions/${result.execution.executionId}`);
                return `已重建执行 ${result.execution.executionId}（${result.plan.reason}）`;
              })
            }
          >
            {busy === "retry" ? <Loader2 className="size-4 animate-spin" /> : <RefreshCw className="size-4" />}
            重试
          </Button>
          <Button
            variant="outline"
            size="sm"
            disabled={busy !== undefined}
            onClick={() =>
              void run("resume", async () => {
                await api.resumeExecution(executionId);
                return "已恢复执行";
              })
            }
          >
            {busy === "resume" ? <Loader2 className="size-4 animate-spin" /> : <RotateCcw className="size-4" />}
            恢复
          </Button>
        </div>
      </div>

      {detail.error ? <ErrorNotice error={detail.error} onRetry={detail.refresh} /> : null}

      <div className="grid gap-4 lg:grid-cols-[1.7fr_1fr]">
        <Card className="flex min-h-[520px] flex-col border-border/70 bg-card/60">
          <CardHeader>
            <CardTitle className="text-base">事件时间线</CardTitle>
            <CardDescription>
              服务端持久化的统一事件流；断线后按 sequence 补齐，界面不会把“最后一条消息”当成成功。
            </CardDescription>
          </CardHeader>
          <CardContent className="min-h-0 flex-1">
            {timeline.length === 0 ? (
              <EmptyState
                icon={ScrollText}
                title="没有事件"
                description="该执行尚未产生任何事件，或事件历史已被清理。"
              />
            ) : (
              <ScrollArea className="h-[440px] pr-3">
                <ol className="space-y-2">
                  {timeline.map(event => {
                    const body = agentMessageText(event.data);
                    const toolName = toolNameOf(event.data);
                    // One content line per row: what the agent said, or what the tool returned, or
                    // the payload's own summary — never the same text twice.
                    const content =
                      body ??
                      toolResultText(event.data) ??
                      summarizePayload(event.data);
                    // Pi streams a lifecycle event per chunk (`message_update` fires for every text
                    // and thinking delta), so a single run can emit well over a hundred of them.
                    // Rendered as full cards they bury the handful of events that carry meaning.
                    if (event.type === "agent.runtime.unknown") {
                      return (
                        <li
                          key={`${event.id}-${event.sequence}`}
                          className="flex items-center gap-2 rounded-md border border-border/40 bg-background/20 px-3 py-1"
                        >
                          <span className="font-mono text-[11px] text-muted-foreground/70">
                            #{event.sequence}
                          </span>
                          <span className="truncate text-[11px] text-muted-foreground">
                            {eventLabel(event.type, event.data)}
                          </span>
                          <span className="truncate font-mono text-[10px] text-muted-foreground/60">
                            {runtimeEventName(event.data)}
                          </span>
                          <span className="ml-auto shrink-0 font-mono text-[10px] text-muted-foreground/60">
                            {formatClock(event.timestamp)}
                          </span>
                        </li>
                      );
                    }
                    return (
                      <li
                        key={`${event.id}-${event.sequence}`}
                        className="rounded-lg border border-border/60 bg-background/40 p-3"
                      >
                        <div className="flex items-center justify-between gap-3">
                          <div className="flex min-w-0 items-center gap-2">
                            <span
                              className={`font-mono text-xs ${EVENT_TONE[event.type] ?? "text-muted-foreground"}`}
                            >
                              #{event.sequence}
                            </span>
                            <span className="truncate text-sm">{eventLabel(event.type, event.data)}</span>
                            {runtimeEventName(event.data) ? (
                              <span className="hidden shrink-0 rounded border border-border/60 px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground sm:inline">
                                {runtimeEventName(event.data)}
                              </span>
                            ) : null}
                          </div>
                          <span className="shrink-0 font-mono text-[11px] text-muted-foreground">
                            {formatClock(event.timestamp)}
                          </span>
                        </div>
                        {event.type === "agent.message.completed" ? (
                          body ? (
                            <p className="mt-2 max-h-72 overflow-auto rounded-md border border-border/60 bg-background/60 p-2 text-xs leading-relaxed whitespace-pre-wrap text-foreground/90">
                              {body}
                            </p>
                          ) : null
                        ) : content ? (
                          <p className="mt-1 line-clamp-3 text-xs text-muted-foreground">
                            {toolName && content !== toolName ? `${toolName} · ${content}` : content}
                          </p>
                        ) : null}
                        <details className="mt-2">
                          <summary className="cursor-pointer text-[11px] text-muted-foreground select-none">
                            原始数据
                          </summary>
                          <pre className="mt-2 max-h-56 overflow-auto rounded-md border border-border/60 bg-background/70 p-2 text-[11px] whitespace-pre-wrap text-muted-foreground">
                            {previewPayload(event.data, 4000)}
                          </pre>
                        </details>
                      </li>
                    );
                  })}
                </ol>
              </ScrollArea>
            )}
          </CardContent>
        </Card>

        <div className="space-y-4">
          <Card className="border-border/70 bg-card/60">
            <CardHeader>
              <CardTitle className="flex items-center gap-2 text-base">
                <MessageSquare className="size-4 text-muted-foreground" />
                Agent 输出
              </CardTitle>
              <CardDescription>
                运行时产生的最后一条助手消息原文；执行被取消时，这里是中止前的内容。
              </CardDescription>
            </CardHeader>
            <CardContent>
              {lastOutput ? (
                <ScrollArea className="max-h-72">
                  <p className="text-xs leading-relaxed whitespace-pre-wrap text-foreground/90">
                    {lastOutput}
                  </p>
                </ScrollArea>
              ) : (
                <p className="text-xs text-muted-foreground">该执行还没有产生助手输出。</p>
              )}
            </CardContent>
          </Card>

          <Card className="border-border/70 bg-card/60">
            <CardHeader>
              <CardTitle className="flex items-center gap-2 text-base">
                <CircleCheck className="size-4 text-muted-foreground" />
                完成门禁
              </CardTitle>
              <CardDescription>“完成”是一个需要证据的结论。</CardDescription>
            </CardHeader>
            <CardContent>
              {detail.data?.completion ? (
                <div className="space-y-2">
                  <StatusBadge status={detail.data.completion.allowed ? "passed" : "failed"} />
                  <p className="text-xs text-muted-foreground">{detail.data.completion.reason}</p>
                </div>
              ) : (
                <p className="text-xs text-muted-foreground">读取中…</p>
              )}
            </CardContent>
          </Card>

          <Card className="border-border/70 bg-card/60">
            <CardHeader>
              <CardTitle className="flex items-center gap-2 text-base">
                <KeyRound className="size-4 text-muted-foreground" />
                权限审批
              </CardTitle>
              <CardDescription>Runtime 提问，CodeWeaver 决定；未决请求不会被自动放行。</CardDescription>
            </CardHeader>
            <CardContent className="space-y-3">
              {approvals.length === 0 ? (
                <p className="text-xs text-muted-foreground">当前没有待审批的权限请求。</p>
              ) : (
                approvals.map(approval => (
                  <div
                    key={approval.requestId}
                    className="space-y-2 rounded-lg border border-amber-500/30 bg-amber-500/5 p-3"
                  >
                    <p className="font-mono text-xs">{approval.requestId}</p>
                    <p className="text-xs text-muted-foreground">
                      {(approval.tool ?? summarizePayload(approval.payload)) || "未说明操作"}
                    </p>
                    <pre className="max-h-32 overflow-auto rounded-md border border-border/60 bg-background/70 p-2 text-[11px] whitespace-pre-wrap text-muted-foreground">
                      {previewPayload(approval.payload, 1200)}
                    </pre>
                    <div className="flex gap-2">
                      <Button
                        size="sm"
                        disabled={busy !== undefined}
                        onClick={() =>
                          void run(`approve-${approval.requestId}`, async () => {
                            await api.decideApproval(executionId, approval.requestId, "approve");
                            detail.refresh();
                            return "已批准";
                          })
                        }
                      >
                        <CircleCheck className="size-4" />
                        批准
                      </Button>
                      <Button
                        size="sm"
                        variant="outline"
                        disabled={busy !== undefined}
                        onClick={() =>
                          void run(`deny-${approval.requestId}`, async () => {
                            await api.decideApproval(executionId, approval.requestId, "deny");
                            detail.refresh();
                            return "已拒绝";
                          })
                        }
                      >
                        <CircleX className="size-4" />
                        拒绝
                      </Button>
                    </div>
                  </div>
                ))
              )}
            </CardContent>
          </Card>

          <Card className="border-border/70 bg-card/60">
            <CardHeader>
              <CardTitle className="flex items-center gap-2 text-base">
                <FileDiff className="size-4 text-muted-foreground" />
                检查点
              </CardTitle>
            </CardHeader>
            <CardContent>
              {detail.data ? (
                <dl className="grid grid-cols-2 gap-x-3 gap-y-2 text-xs">
                  <Field label="终态" value={detail.data.checkpoint.terminal ? "是" : "否"} />
                  <Field label="终态事件" value={detail.data.checkpoint.terminalEventType ?? "-"} />
                  <Field label="最后序号" value={String(detail.data.checkpoint.lastSequence)} />
                  <Field label="事件数" value={String(detail.data.checkpoint.eventCount)} />
                  <Field label="上下文快照" value={detail.data.checkpoint.contextSnapshotId ?? "-"} />
                  <Field label="策略快照" value={detail.data.checkpoint.policySnapshotId ?? "-"} />
                  <Field label="Git 基线" value={detail.data.checkpoint.gitBaseSha ?? "-"} />
                  <Field label="缺口" value={detail.data.checkpoint.gaps.join(", ") || "无"} />
                  <Field label="创建时间" value={formatTime(detail.data.checkpoint.createdAt)} />
                  <Field
                    label="变更记录"
                    value={detail.data.checkpoint.appliedChangeRecordIds.join(", ") || "-"}
                  />
                </dl>
              ) : (
                <p className="text-xs text-muted-foreground">读取中…</p>
              )}
            </CardContent>
          </Card>

          <Card className="border-border/70 bg-card/60">
            <CardHeader>
              <CardTitle className="flex items-center gap-2 text-base">
                <Gauge className="size-4 text-muted-foreground" />
                用量与技能
              </CardTitle>
            </CardHeader>
            <CardContent className="space-y-3">
              <Tabs defaultValue="usage">
                <TabsList className="w-full">
                  <TabsTrigger value="usage" className="flex-1">
                    用量
                  </TabsTrigger>
                  <TabsTrigger value="skills" className="flex-1">
                    技能
                  </TabsTrigger>
                </TabsList>
                <TabsContent value="usage" className="mt-3">
                  {usage.data?.usage ? (
                    <dl className="grid grid-cols-2 gap-x-3 gap-y-2 text-xs">
                      <Field label="模型" value={usage.data.usage.model} />
                      <Field label="总 Token" value={formatTokens(usage.data.usage.totalTokens)} />
                      <Field label="输入" value={formatTokens(usage.data.usage.promptTokens)} />
                      <Field label="输出" value={formatTokens(usage.data.usage.completionTokens)} />
                      <Field label="耗时" value={formatDuration(usage.data.usage.durationMs)} />
                      <Field label="结束于" value={formatTime(usage.data.usage.finishedAt)} />
                    </dl>
                  ) : (
                    <p className="text-xs text-muted-foreground">
                      {usage.error ? usage.error.message : "该执行没有用量记录。"}
                    </p>
                  )}
                </TabsContent>
                <TabsContent value="skills" className="mt-3">
                  {skills.data?.bindings && skills.data.bindings.length > 0 ? (
                    <ul className="space-y-2">
                      {skills.data.bindings.map(binding => {
                        const key = `${binding.skillId}@${binding.version}`;
                        const manifest = registry.data?.skills.find(
                          skill => skill.skillId === binding.skillId && skill.version === binding.version
                        );
                        // The delivery record says the body reached the runtime. A binding without
                        // one is a skill that was authorized and then dropped.
                        const delivery = deliveries.find(
                          item => item.skillId === binding.skillId && item.version === binding.version
                        );
                        return (
                          <li
                            key={`${binding.skillId}-${binding.version}`}
                            className="space-y-1.5 rounded-md border border-border/60 bg-background/40 px-2 py-1.5 text-xs"
                          >
                            <div className="flex items-center justify-between gap-2">
                              <span className="flex items-center gap-1.5 truncate">
                                <Sparkles className="size-3.5 text-muted-foreground" />
                                {key}
                              </span>
                              <span className="text-muted-foreground">
                                {delivery ? `已注入 · ${String(delivery.deliveryMode ?? "")}` : "未记录注入"}
                              </span>
                            </div>
                            {manifest ? (
                              <pre className="max-h-40 overflow-auto whitespace-pre-wrap rounded bg-background/60 p-2 text-[11px] text-muted-foreground">
                                {manifest.content}
                              </pre>
                            ) : (
                              <p className="text-[11px] text-muted-foreground">
                                技能库中没有这一版本，无法显示它当时携带的正文。
                              </p>
                            )}
                          </li>
                        );
                      })}
                    </ul>
                  ) : (
                    <p className="text-xs text-muted-foreground">
                      {skills.error ? skills.error.message : "该执行没有绑定技能。"}
                    </p>
                  )}
                </TabsContent>
              </Tabs>
            </CardContent>
          </Card>

          <Card className="border-border/70 bg-card/60">
            <CardHeader>
              <CardTitle className="flex items-center gap-2 text-base">
                <Clock className="size-4 text-muted-foreground" />
                恢复语义
              </CardTitle>
            </CardHeader>
            <CardContent className="space-y-2 text-xs text-muted-foreground">
              <p>
                <Plug className="mr-1 inline size-3.5" />
                重试会新建执行实例并记录来源；恢复只在 Runtime 明确支持续跑时可用，否则会返回 501。
              </p>
              <Separator className="bg-border/60" />
              <p>本页不会把“收到最后一条消息”当作执行成功，判定依据是终态与评估证据。</p>
            </CardContent>
          </Card>
        </div>
      </div>
    </div>
  );
}

function Field({ label, value }: { label: string; value: string }) {
  return (
    <div className="min-w-0">
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="truncate font-mono" title={value}>
        {value}
      </dd>
    </div>
  );
}
