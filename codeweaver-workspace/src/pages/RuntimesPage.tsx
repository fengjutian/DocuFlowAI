import { useState } from "react";
import { HeartPulse, Loader2, Server } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { EmptyState, ErrorNotice } from "@/components/common/EmptyState";
import { StatusBadge } from "@/components/common/StatusBadge";
import { api } from "@/lib/api";
import { formatTime, shortId } from "@/lib/format";
import { useApiData } from "@/hooks/useApiData";
import type { RuntimeHealth } from "@/lib/types";

/**
 * Runtime registry view.
 *
 * Health is not cached in the UI: `POST /api/v1/runtimes/{id}/health-check` asks the server to probe
 * the runtime right now, and the answer shown is the server's own report (including the error text
 * when the runtime is unreachable).
 */
export function RuntimesPage() {
  const runtimes = useApiData(() => api.listRuntimes(), []);
  const [health, setHealth] = useState<Record<string, RuntimeHealth>>({});
  const [checking, setChecking] = useState<string | undefined>(undefined);

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
