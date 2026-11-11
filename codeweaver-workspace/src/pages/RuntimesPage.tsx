import { Fragment, useState } from "react";
import { HeartPulse, Loader2, Server, Trophy } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Progress } from "@/components/ui/progress";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { EmptyState, ErrorNotice } from "@/components/common/EmptyState";
import { StatusBadge } from "@/components/common/StatusBadge";
import { api } from "@/lib/api";
import { formatTime, shortId } from "@/lib/format";
import { useApiData } from "@/hooks/useApiData";
import type { RuntimeHealth, RuntimeScore } from "@/lib/types";

/**
 * Runtime registry view.
 *
 * Health is not cached in the UI: `POST /api/v1/runtimes/{id}/health-check` asks the server to probe
 * the runtime right now, and the answer shown is the server's own report (including the error text
 * when the runtime is unreachable).
 *
 * The score table below is the same discipline applied to quality: the server scores the runtimes
 * from what the journal recorded, and the UI shows that verdict — including the dimensions nobody
 * could measure, which are shown as unmeasured rather than as zero.
 */
export function RuntimesPage() {
  const runtimes = useApiData(() => api.listRuntimes(), []);
  const scores = useApiData(() => api.runtimeScores(), []);
  const scoreReport = scores.data;
  const [health, setHealth] = useState<Record<string, RuntimeHealth>>({});
  const [checking, setChecking] = useState<string | undefined>(undefined);
  const [expanded, setExpanded] = useState<string | undefined>(undefined);

  const check = async (runtimeId: string) => {
    setChecking(runtimeId);
    try {
      const report = await api.healthCheck(runtimeId);
      setHealth(current => ({ ...current, [runtimeId]: report }));
      if (report.healthy) toast.success(`${runtimeId} 健康检查通过`);
      else toast.error(`${runtimeId} 不可用：${report.error ?? "未说明原因"}`);
    } catch (error) {
      // A 503 is the documented answer for an unhealthy runtime; the body still carries the report.
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      setChecking(undefined);
    }
  };

  return (
    <div className="space-y-4">
      <Card className="border-border/70 bg-card/60">
        <CardHeader>
          <CardTitle className="text-base">Runtime Registry</CardTitle>
          <CardDescription>
            运行时由后端注册与探活；前端只发起检查并展示服务端结论。
          </CardDescription>
        </CardHeader>
        <CardContent>
          {runtimes.error ? <ErrorNotice error={runtimes.error} onRetry={runtimes.refresh} /> : null}

          {runtimes.loading ? <p className="text-sm text-muted-foreground">读取中…</p> : null}

          {runtimes.data && runtimes.data.runtimes.length === 0 ? (
            <EmptyState icon={Server} title="没有已注册的 Runtime" />
          ) : null}

          {runtimes.data && runtimes.data.runtimes.length > 0 ? (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>ID</TableHead>
                  <TableHead>类型</TableHead>
                  <TableHead>版本</TableHead>
                  <TableHead>健康</TableHead>
                  <TableHead>探活时间</TableHead>
                  <TableHead className="text-right">操作</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {runtimes.data.runtimes.map(runtime => {
                  const report = health[runtime.id];
                  return (
                    <TableRow key={runtime.id}>
                      <TableCell className="font-mono text-xs">{runtime.id}</TableCell>
                      <TableCell className="text-xs">{runtime.runtimeType}</TableCell>
                      <TableCell className="text-xs text-muted-foreground">
                        {runtime.version ?? "-"}
                      </TableCell>
                      <TableCell>
                        {report ? (
                          <div className="space-y-1">
                            <StatusBadge status={report.healthy ? "healthy" : "failed"} />
                            {report.error ? (
                              <p className="max-w-md text-xs text-muted-foreground">{report.error}</p>
                            ) : null}
                          </div>
                        ) : (
                          <span className="text-xs text-muted-foreground">未检查</span>
                        )}
                      </TableCell>
                      <TableCell className="text-xs text-muted-foreground">
                        {report ? formatTime(report.checkedAt) : "-"}
                      </TableCell>
                      <TableCell className="text-right">
                        <div className="flex items-center justify-end gap-2">
                          <Button
                            size="sm"
                            variant="outline"
                            onClick={() => void check(runtime.id)}
                            disabled={checking === runtime.id}
                          >
                            {checking === runtime.id ? (
                              <Loader2 className="size-4 animate-spin" />
                            ) : (
                              <HeartPulse className="size-4" />
                            )}
                            健康检查
                          </Button>
                          <Tooltip>
                            <TooltipTrigger asChild>
                              <span>
                                <Button size="sm" variant="ghost" disabled>
                                  禁用
                                </Button>
                              </span>
                            </TooltipTrigger>
                            <TooltipContent>后端尚未实现 disable 接口（设计 12.1 已定义，未落地）</TooltipContent>
                          </Tooltip>
                        </div>
                      </TableCell>
                    </TableRow>
                  );
                })}
              </TableBody>
            </Table>
          ) : null}
        </CardContent>
      </Card>

      <Card className="border-border/70 bg-card/60">
        <CardHeader>
          <CardTitle className="text-base">运行时能力评分</CardTitle>
          <CardDescription>
            分数只来自 journal 里的真实记录：成功/失败、取消是否由运行时确认、工具与权限事件、
            耗时与 token、以及运行时自己声明的能力。没有任何证据的维度显示为「未测量」，不按 0 计。
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-3">
          {scores.error ? <ErrorNotice error={scores.error} onRetry={scores.refresh} /> : null}
          {scores.loading ? <p className="text-sm text-muted-foreground">读取中…</p> : null}

          {scoreReport && scoreReport.runtimes.length === 0 ? (
            <EmptyState icon={Trophy} title="还没有可评分的运行时记录" />
          ) : null}

          {scoreReport && scoreReport.runtimes.length > 0 ? (
            <>
              <p className="text-xs text-muted-foreground">
                journal 共 {scoreReport.executionsInJournal} 条执行记录
                {scoreReport.unattributedExecutions > 0
                  ? `，其中 ${scoreReport.unattributedExecutions} 条没有记录运行时归属`
                  : ""}
              </p>
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>运行时</TableHead>
                    <TableHead>评分</TableHead>
                    <TableHead>成功率</TableHead>
                    <TableHead>取消确认</TableHead>
                    <TableHead>能力兑现</TableHead>
                    <TableHead>评测通过</TableHead>
                    <TableHead>平均耗时</TableHead>
                    <TableHead>tokens/成功</TableHead>
                    <TableHead>声明能力</TableHead>
                    <TableHead>观测能力</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {scoreReport.comparison.rows.map(row => {
                    const detail = scoreReport.runtimes.find(item => item.runtimeId === row.runtimeId);
                    const open = expanded === row.runtimeId;
                    return (
                      <Fragment key={row.runtimeId}>
                        <TableRow
                          key={row.runtimeId}
                          className="cursor-pointer"
                          onClick={() => setExpanded(open ? undefined : row.runtimeId)}
                        >
                          <TableCell className="font-mono text-xs">
                            {row.runtimeId}
                            <span className="ml-2 text-muted-foreground">{row.runtimeType}</span>
                          </TableCell>
                          <TableCell className="text-xs font-medium">
                            {row.score === null ? (
                              <span className="text-muted-foreground">未测量</span>
                            ) : (
                              <div className="space-y-1">
                                <div className="flex items-center gap-2">
                                  <Progress value={row.score} className="h-1.5 w-16" />
                                  <span>{row.score}</span>
                                </div>
                                {detail ? (
                                  <p className="text-[10px] text-muted-foreground">
                                    {detail.dimensions.filter(item => item.measured).length}/
                                    {detail.dimensions.length} 维度有证据
                                  </p>
                                ) : null}
                              </div>
                            )}
                          </TableCell>
                          <TableCell className="text-xs">{formatRate(row.successRate)}</TableCell>
                          <TableCell className="text-xs">{formatRate(row.cancelConfirmationRate)}</TableCell>
                          <TableCell className="text-xs">{formatRate(row.capabilityRealization)}</TableCell>
                          <TableCell className="text-xs">{formatRate(row.evaluationPassRate)}</TableCell>
                          <TableCell className="text-xs">
                            {row.averageDurationMs === null ? "-" : `${row.averageDurationMs} ms`}
                          </TableCell>
                          <TableCell className="text-xs">
                            {row.tokensPerSuccessfulExecution === null
                              ? "-"
                              : row.tokensPerSuccessfulExecution}
                          </TableCell>
                          <TableCell className="text-xs text-muted-foreground">
                            {row.declaredCapabilities.join(", ") || "-"}
                          </TableCell>
                          <TableCell className="text-xs text-muted-foreground">
                            {row.observedCapabilities.join(", ") || "-"}
                          </TableCell>
                        </TableRow>
                        {open && detail ? (
                          <TableRow>
                            <TableCell colSpan={10} className="bg-background/40">
                              <ScoreDetail score={detail} />
                            </TableCell>
                          </TableRow>
                        ) : null}
                      </Fragment>
                    );
                  })}
                </TableBody>
              </Table>
            </>
          ) : null}
        </CardContent>
      </Card>

      <Card className="border-border/70 bg-card/60">
        <CardHeader>
          <CardTitle className="text-base">Runtime 元数据</CardTitle>
          <CardDescription>注册时携带的原始元数据，用于排查适配器配置问题。</CardDescription>
        </CardHeader>
        <CardContent className="space-y-3">
          {runtimes.data?.runtimes.map(runtime => (
            <div key={runtime.id} className="rounded-lg border border-border/60 bg-background/40 p-3">
              <p className="text-xs font-medium">
                {runtime.id}
                <span className="ml-2 font-mono text-muted-foreground">{shortId(runtime.id, 24)}</span>
              </p>
              <pre className="mt-2 overflow-x-auto text-xs text-muted-foreground">
                {JSON.stringify(runtime.metadata, null, 2)}
              </pre>
            </div>
          ))}
        </CardContent>
      </Card>
    </div>
  );
}

/** The dimensions behind one runtime's score, with the reason each one is weighted as it is. */
function ScoreDetail({ score }: { score: RuntimeScore }) {
  return (
    <div className="space-y-2 py-2">
      <p className="text-xs text-muted-foreground">
        执行 {score.metrics.executions} 次：成功 {score.metrics.succeeded} / 失败 {score.metrics.failed} /
        取消 {score.metrics.cancelled} / 未结束 {score.metrics.unfinished}；工具事件{" "}
        {score.metrics.toolEventCount} 次，权限事件 {score.metrics.permissionEventCount} 次，
        取消请求 {score.metrics.cancelRequests} 次（运行时确认 {score.metrics.cancelConfirmed} 次）。
      </p>
      {score.metrics.declaredNotObserved.length > 0 ? (
        <p className="text-xs text-muted-foreground">
          声明了但记录里没看到：{score.metrics.declaredNotObserved.join(", ")}
        </p>
      ) : null}
      {score.metrics.observedNotDeclared.length > 0 ? (
        <p className="text-xs text-muted-foreground">
          记录里看到但没声明：{score.metrics.observedNotDeclared.join(", ")}
        </p>
      ) : null}
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>维度</TableHead>
            <TableHead>权重</TableHead>
            <TableHead>实际权重</TableHead>
            <TableHead>得分</TableHead>
            <TableHead>口径</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {score.dimensions.map(dimension => (
            <TableRow key={dimension.key}>
              <TableCell className="text-xs">{dimension.key}</TableCell>
              <TableCell className="text-xs">{dimension.weight}</TableCell>
              <TableCell className="text-xs">
                {dimension.measured ? dimension.effectiveWeight.toFixed(3) : "未测量"}
              </TableCell>
              <TableCell className="text-xs">
                {dimension.measured && dimension.score !== null
                  ? dimension.score.toFixed(3)
                  : "未测量"}
              </TableCell>
              <TableCell className="max-w-xl text-xs text-muted-foreground">{dimension.detail}</TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  );
}

function formatRate(value: number | null): string {
  if (value === null) return "未测量";
  return `${(value * 100).toFixed(0)}%`;
}
