import { useNavigate } from "react-router-dom";
import {
  Activity,
  Boxes,
  Coins,
  KeyRound,
  Play,
  Server,
  Sparkles
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { ScrollArea } from "@/components/ui/scroll-area";
import { EmptyState, ErrorNotice } from "@/components/common/EmptyState";
import { StatCard } from "@/components/common/StatCard";
import { StatusBadge } from "@/components/common/StatusBadge";
import { api } from "@/lib/api";
import { formatTokens, relativeTime, shortId } from "@/lib/format";
import { useApiData } from "@/hooks/useApiData";
import { useExecutionStream, usePendingPermissionCount } from "@/state/execution-stream";

/**
 * Workspace overview.
 *
 * Every number here comes from the server: runtimes from the registry, usage from the ledger,
 * executions and pending permissions from the live event stream. The server exposes no execution
 * list endpoint, so the stream is the honest source for "what has been running".
 */
export function OverviewPage({ onNewTask }: { onNewTask: () => void }) {
  const navigate = useNavigate();
  const { executions, streamStatus } = useExecutionStream();
  const pendingPermissions = usePendingPermissionCount();

  const runtimes = useApiData(() => api.listRuntimes(), []);
  const usage = useApiData(() => api.getUsage(), []);
  const skills = useApiData(() => api.listSkills(), []);

  const activeCount = executions.filter(item =>
    ["running", "starting", "queued", "waiting_approval", "cancelling"].includes(item.status)
  ).length;

  return (
    <div className="space-y-6">
      <section className="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
        <StatCard
          label="已注册 Runtime"
          value={runtimes.data ? String(runtimes.data.runtimes.length) : "-"}
          hint={runtimes.data?.runtimes.map(item => item.runtimeType).join(" · ") ?? "读取中"}
          icon={Server}
          loading={runtimes.loading}
        />
        <StatCard
          label="活跃执行"
          value={String(activeCount)}
          hint={`事件流：${streamStatus === "open" ? "实时" : streamStatus}`}
          icon={Activity}
          tone="brand"
        />
        <StatCard
          label="待审批权限"
          value={String(pendingPermissions)}
          hint="来自事件流的未决请求"
          icon={KeyRound}
        />
        <StatCard
          label="累计 Token"
          value={usage.data ? formatTokens(usage.data.summary.totalTokens) : "-"}
          hint={usage.data ? `${usage.data.summary.executions} 次执行` : "用量未接入"}
          icon={Coins}
          loading={usage.loading}
        />
      </section>

      {runtimes.error ? <ErrorNotice error={runtimes.error} onRetry={runtimes.refresh} /> : null}

      <div className="grid gap-6 lg:grid-cols-[1.1fr_1.6fr]">
        <Card className="border-border/70 bg-card/60">
          <CardHeader>
            <CardTitle className="flex items-center gap-2 text-base">
              <Boxes className="size-4 text-muted-foreground" />
              Runtime
            </CardTitle>
            <CardDescription>执行后端来自 Runtime Registry，由后端注册，前端不新增。</CardDescription>
          </CardHeader>
          <CardContent className="space-y-3">
            {runtimes.loading ? <p className="text-sm text-muted-foreground">读取中…</p> : null}
            {!runtimes.loading && runtimes.data && runtimes.data.runtimes.length === 0 ? (
              <EmptyState
                icon={Server}
                title="没有已注册的 Runtime"
                description="后端启动时会注册 hermes-local；若无记录，说明连接的不是预期的服务端。"
              />
            ) : null}
            {runtimes.data?.runtimes.map(runtime => (
              <div
                key={runtime.id}
                className="flex items-center justify-between gap-3 rounded-lg border border-border/60 bg-background/40 px-3 py-2"
              >
                <div className="min-w-0">
                  <p className="truncate text-sm font-medium">{runtime.id}</p>
                  <p className="truncate text-xs text-muted-foreground">
                    {runtime.runtimeType} · {runtime.version ?? "未固定版本"}
                  </p>
                </div>
                <StatusBadge status="running" />
              </div>
            ))}

            <div className="flex items-center justify-between rounded-lg border border-border/60 bg-background/40 px-3 py-2">
              <div className="min-w-0">
                <p className="truncate text-sm font-medium">已注册技能</p>
                <p className="truncate text-xs text-muted-foreground">
                  {skills.data ? `${skills.data.skills.length} 个技能清单` : "读取中…"}
                </p>
              </div>
              <Sparkles className="size-4 text-muted-foreground" />
            </div>
          </CardContent>
        </Card>

        <Card className="flex min-h-[320px] flex-col border-border/70 bg-card/60">
          <CardHeader className="flex-row items-center justify-between space-y-0">
            <div>
              <CardTitle className="text-base">近期执行</CardTitle>
              <CardDescription>来自实时事件流；后端暂未提供执行列表接口。</CardDescription>
            </div>
            <Button size="sm" onClick={onNewTask}>
              <Play className="size-4" />
              新建执行
            </Button>
          </CardHeader>
          <CardContent className="min-h-0 flex-1">
            {executions.length === 0 ? (
              <EmptyState
                icon={Activity}
                title="暂时没有执行事件"
                description="启动一次执行后，事件会实时出现在这里。"
                action={
                  <Button size="sm" variant="outline" onClick={onNewTask}>
                    <Play className="size-4" />
                    新建执行
                  </Button>
                }
              />
            ) : (
              <ScrollArea className="h-[260px] pr-3">
                <ul className="space-y-2">
                  {executions.map(item => (
                    <li key={item.executionId}>
                      <button
                        type="button"
                        onClick={() => navigate(`/executions/${item.executionId}`)}
                        className="flex w-full items-center justify-between gap-3 rounded-lg border border-border/60 bg-background/40 px-3 py-2 text-left transition-colors hover:border-brand/50 hover:bg-brand/5"
                      >
                        <div className="min-w-0">
                          <p className="truncate font-mono text-xs">{item.executionId}</p>
                          <p className="truncate text-xs text-muted-foreground">
                            {item.taskId ? `${item.taskId} · ` : ""}
                            {item.eventCount} 条事件 · {relativeTime(item.lastTimestamp)}
                          </p>
                        </div>
                        <StatusBadge status={item.status} />
                      </button>
                    </li>
                  ))}
                </ul>
              </ScrollArea>
            )}
          </CardContent>
        </Card>
      </div>

      <Card className="border-border/70 bg-card/60">
        <CardHeader>
          <CardTitle className="text-base">控制平面说明</CardTitle>
          <CardDescription>本页展示的每个状态都来自服务端，界面本身不做判定。</CardDescription>
        </CardHeader>
        <CardContent className="grid gap-3 text-xs text-muted-foreground sm:grid-cols-3">
          <p>
            DocuFlow 是唯一的任务控制平面：决定执行什么任务、是否允许写入文件、是否可以跳过验收。
          </p>
          <p>
            Agent Runtime 只是可替换的执行平面。执行完成与否以终态和评估证据为准，不以“收到最后一条消息”为准。
          </p>
          <p>
            所有代码变更经 Code Engine，所有敏感操作经 Policy Engine，全部过程可审计、可恢复、可追溯。
          </p>
        </CardContent>
      </Card>

      {usage.error ? (
        <p className="text-xs text-muted-foreground">用量未接入：{usage.error.message}</p>
      ) : null}
      {executions.length > 0 ? (
        <p className="text-xs text-muted-foreground">
          最近一次执行：{shortId(executions[0]?.executionId, 12)} · {relativeTime(executions[0]?.lastTimestamp)}
        </p>
      ) : null}
    </div>
  );
}
