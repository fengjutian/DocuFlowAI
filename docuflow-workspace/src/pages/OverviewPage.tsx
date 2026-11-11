import { useNavigate } from "react-router-dom";
import {
  Activity, ArrowRight, Boxes, CheckCircle2, Coins, FileCode2, KeyRound,
  Play, Radio, Server, ShieldCheck, Sparkles
} from "lucide-react";
import { cn } from "cn";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { ScrollArea } from "@/components/ui/scroll-area";
import { EmptyState, ErrorNotice } from "@/components/common/EmptyState";
import { StatusBadge } from "@/components/common/StatusBadge";
import { api } from "@/lib/api";
import { formatTokens, relativeTime, shortId } from "@/lib/format";
import { useApiData } from "@/hooks/useApiData";
import { useExecutionStream, usePendingPermissionCount } from "@/state/execution-stream";

const ACTIVE_STATUSES = ["running", "starting", "queued", "waiting_approval", "cancelling"];

/** Task-oriented overview. State comes from the backend or live event stream. */
export function OverviewPage({ onNewTask }: { onNewTask: () => void }) {
  const navigate = useNavigate();
  const { executions, streamStatus } = useExecutionStream();
  const pendingPermissions = usePendingPermissionCount();
  const runtimes = useApiData(() => api.listRuntimes(), []);
  const usage = useApiData(() => api.getUsage(), []);
  const skills = useApiData(() => api.listSkills(), []);

  const activeCount = executions.filter(item => ACTIVE_STATUSES.includes(item.status)).length;
  const streamOnline = streamStatus === "open";
  const latestExecution = executions[0];
  const metrics = [
    {
      label: "可用 Runtime",
      value: runtimes.data ? String(runtimes.data.runtimes.length) : "—",
      detail: runtimes.data?.runtimes.map(item => item.runtimeType).join(" · ") || "等待注册",
      icon: Server, accent: "text-violet-600 dark:text-violet-400", glow: "from-violet-500/20"
    },
    {
      label: "活跃执行", value: String(activeCount),
      detail: streamOnline ? "事件流实时同步" : `事件流 ${streamStatus}`,
      icon: Activity, accent: "text-sky-600 dark:text-sky-400", glow: "from-sky-500/20"
    },
    {
      label: "待审批", value: String(pendingPermissions),
      detail: pendingPermissions > 0 ? "需要你的决策" : "当前没有阻塞项",
      icon: KeyRound,
      accent: pendingPermissions > 0 ? "text-amber-600 dark:text-amber-400" : "text-emerald-600 dark:text-emerald-400",
      glow: pendingPermissions > 0 ? "from-amber-500/20" : "from-emerald-500/20"
    },
    {
      label: "累计 Token",
      value: usage.data ? formatTokens(usage.data.summary.totalTokens) : "—",
      detail: usage.data ? `${usage.data.summary.executions} 次已记账执行` : "等待用量数据",
      icon: Coins, accent: "text-fuchsia-600 dark:text-fuchsia-400", glow: "from-fuchsia-500/20"
    }
  ];

  return (
    <div className="space-y-5 pb-4">
      <section className="relative overflow-hidden rounded-2xl border border-border/70 bg-card/70 p-5 shadow-sm backdrop-blur sm:p-6">
        <div className="pointer-events-none absolute inset-0 bg-[radial-gradient(circle_at_15%_0%,color-mix(in_oklch,var(--brand)_18%,transparent),transparent_40%)]" />
        <div className="relative flex flex-col justify-between gap-5 lg:flex-row lg:items-center">
          <div className="max-w-2xl">
            <div className="mb-3 flex items-center gap-2 text-xs font-medium text-brand">
              <span className="relative flex size-2">
                <span className={cn("absolute inline-flex size-full animate-ping rounded-full opacity-60", streamOnline ? "bg-emerald-400" : "bg-amber-400")} />
                <span className={cn("relative inline-flex size-2 rounded-full", streamOnline ? "bg-emerald-400" : "bg-amber-400")} />
              </span>
              {streamOnline ? "控制平面在线" : "控制平面连接中"}
            </div>
            <h2 className="text-2xl font-semibold tracking-tight sm:text-3xl">今天要让 Agent 完成什么？</h2>
            <p className="mt-2 max-w-xl text-sm leading-6 text-muted-foreground">
              从任务意图到代码变更、策略审批和验收证据，在同一个可审计工作流中完成。
            </p>
          </div>
          <div className="flex shrink-0 flex-wrap gap-2">
            <Button size="lg" onClick={onNewTask} className="shadow-lg shadow-brand/15">
              <Play className="size-4 fill-current" />新建执行
            </Button>
            <Button size="lg" variant="outline" onClick={() => navigate("/executions")}>
              查看全部执行<ArrowRight className="size-4" />
            </Button>
          </div>
        </div>
      </section>

      <section className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
        {metrics.map(metric => (
          <Card key={metric.label} className="group relative overflow-hidden border-border/70 bg-card/65 transition-colors hover:border-border">
            <div className={cn("pointer-events-none absolute inset-x-0 top-0 h-20 bg-gradient-to-b to-transparent opacity-40", metric.glow)} />
            <CardContent className="relative p-4">
              <div className="flex items-center justify-between">
                <p className="text-xs font-medium text-muted-foreground">{metric.label}</p>
                <span className="rounded-lg border border-border/60 bg-background/50 p-2">
                  <metric.icon className={cn("size-4", metric.accent)} />
                </span>
              </div>
              <p className="mt-3 text-3xl font-semibold tracking-tight tabular-nums">{metric.value}</p>
              <p className="mt-1 truncate text-xs text-muted-foreground">{metric.detail}</p>
            </CardContent>
          </Card>
        ))}
      </section>

      {runtimes.error ? <ErrorNotice error={runtimes.error} onRetry={runtimes.refresh} /> : null}

      <section className="grid gap-5 xl:grid-cols-[minmax(0,1.65fr)_minmax(280px,0.85fr)]">
        <Card className="flex min-h-[430px] flex-col border-border/70 bg-card/65">
          <CardHeader className="flex-row items-start justify-between gap-4 space-y-0 border-b border-border/60 pb-4">
            <div>
              <div className="mb-1 flex items-center gap-2">
                <Radio className="size-4 text-brand" /><CardTitle className="text-base">实时执行</CardTitle>
              </div>
              <CardDescription>任务状态和事件由服务端实时推送。</CardDescription>
            </div>
            <span className={cn("rounded-full border px-2.5 py-1 text-[11px]", streamOnline ? "border-emerald-500/25 bg-emerald-500/10 text-emerald-700 dark:text-emerald-400" : "border-amber-500/25 bg-amber-500/10 text-amber-700 dark:text-amber-400")}>
              {streamOnline ? "LIVE" : streamStatus.toUpperCase()}
            </span>
          </CardHeader>
          <CardContent className="min-h-0 flex-1 p-3 sm:p-4">
            {executions.length === 0 ? (
              <EmptyState
                icon={Activity}
                title="准备开始第一次执行"
                description="创建任务后，事件、审批与验收状态会实时汇集到这里。"
                action={<Button className="mt-2" size="sm" onClick={onNewTask}><Play className="size-4 fill-current" />新建执行</Button>}
              />
            ) : (
              <ScrollArea className="h-[330px] pr-3">
                <ul className="space-y-2">
                  {executions.map(item => (
                    <li key={item.executionId}>
                      <button
                        type="button"
                        onClick={() => navigate(`/executions/${item.executionId}`)}
                        className="group flex w-full items-center gap-3 rounded-xl border border-transparent bg-background/35 p-3 text-left transition-all hover:border-brand/25 hover:bg-brand/5"
                      >
                        <span className="flex size-9 shrink-0 items-center justify-center rounded-lg border border-border/60 bg-background/70 text-muted-foreground group-hover:text-brand">
                          <FileCode2 className="size-4" />
                        </span>
                        <div className="min-w-0 flex-1">
                          <div className="flex items-center gap-2">
                            <p className="truncate font-mono text-xs font-medium">{item.taskId || shortId(item.executionId, 14)}</p>
                            <StatusBadge status={item.status} />
                          </div>
                          <p className="mt-1 truncate text-xs text-muted-foreground">
                            {shortId(item.executionId, 12)} · {item.eventCount} 条事件 · {relativeTime(item.lastTimestamp)}
                          </p>
                        </div>
                        <ArrowRight className="size-4 shrink-0 text-muted-foreground/50 transition-transform group-hover:translate-x-0.5 group-hover:text-brand" />
                      </button>
                    </li>
                  ))}
                </ul>
              </ScrollArea>
            )}
          </CardContent>
        </Card>

        <div className="space-y-5">
          <Card className="border-border/70 bg-card/65">
            <CardHeader className="pb-3">
              <CardTitle className="flex items-center gap-2 text-base"><Boxes className="size-4 text-violet-600 dark:text-violet-400" />执行资源</CardTitle>
              <CardDescription>来自 Runtime Registry 的服务端注册信息。</CardDescription>
            </CardHeader>
            <CardContent className="space-y-2">
              {runtimes.loading ? <p className="py-4 text-sm text-muted-foreground">正在读取 Runtime…</p> : null}
              {!runtimes.loading && runtimes.data?.runtimes.length === 0 ? (
                <EmptyState icon={Server} title="暂无 Runtime" description="连接预期服务端后将自动注册。" />
              ) : null}
              {runtimes.data?.runtimes.map(runtime => (
                <div key={runtime.id} className="flex items-center gap-3 rounded-xl border border-border/60 bg-background/35 p-3">
                  <span className="flex size-9 items-center justify-center rounded-lg bg-violet-500/10 text-violet-600 dark:text-violet-400"><Server className="size-4" /></span>
                  <div className="min-w-0 flex-1">
                    <p className="truncate text-sm font-medium">{runtime.id}</p>
                    <p className="truncate text-xs text-muted-foreground">{runtime.runtimeType} · {runtime.version ?? "版本未固定"}</p>
                  </div>
                  <StatusBadge status="running" />
                </div>
              ))}
              <button
                type="button"
                onClick={() => navigate("/skills")}
                className="flex w-full items-center gap-3 rounded-xl border border-border/60 bg-background/35 p-3 text-left transition-colors hover:border-brand/25 hover:bg-brand/5"
              >
                <span className="flex size-9 items-center justify-center rounded-lg bg-sky-500/10 text-sky-600 dark:text-sky-400"><Sparkles className="size-4" /></span>
                <div className="min-w-0 flex-1">
                  <p className="text-sm font-medium">已注册技能</p>
                  <p className="text-xs text-muted-foreground">{skills.data ? `${skills.data.skills.length} 个技能可用` : "正在读取…"}</p>
                </div>
                <ArrowRight className="size-4 text-muted-foreground" />
              </button>
            </CardContent>
          </Card>

          {latestExecution ? (
            <div className="rounded-xl border border-border/60 bg-background/25 px-4 py-3 text-xs text-muted-foreground">
              最近更新：<span className="font-mono text-foreground">{shortId(latestExecution.executionId, 12)}</span> · {relativeTime(latestExecution.lastTimestamp)}
            </div>
          ) : null}
        </div>
      </section>

      <section className="grid gap-3 md:grid-cols-3">
        {[
          { icon: ShieldCheck, title: "策略先行", text: "敏感操作统一经过 Policy Engine 决策与审批。" },
          { icon: CheckCircle2, title: "证据验收", text: "以终态和评估证据判断完成，而非最后一条消息。" },
          { icon: FileCode2, title: "全程可追溯", text: "代码变更经 Code Engine，过程可审计、可恢复。" }
        ].map(item => (
          <div key={item.title} className="flex gap-3 rounded-xl border border-border/60 bg-card/45 p-4">
            <item.icon className="mt-0.5 size-4 shrink-0 text-brand" />
            <div><p className="text-sm font-medium">{item.title}</p><p className="mt-1 text-xs leading-5 text-muted-foreground">{item.text}</p></div>
          </div>
        ))}
      </section>

      {usage.error ? <p className="text-xs text-muted-foreground">用量暂不可用：{usage.error.message}</p> : null}
    </div>
  );
}
