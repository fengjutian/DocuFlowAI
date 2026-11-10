import { Coins, Cpu, Timer } from "lucide-react";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { EmptyState, ErrorNotice } from "@/components/common/EmptyState";
import { StatCard } from "@/components/common/StatCard";
import { api } from "@/lib/api";
import { formatDuration, formatTime, formatTokens } from "@/lib/format";
import { useApiData } from "@/hooks/useApiData";

/**
 * Usage accounting.
 *
 * The ledger records one entry per execution, written only when the runtime reported token counts:
 * an execution without usage data produces no record rather than a fake zero.
 */
export function UsagePage() {
  const usage = useApiData(() => api.getUsage(), []);

  return (
    <div className="space-y-4">
      {usage.error ? <ErrorNotice error={usage.error} onRetry={usage.refresh} /> : null}

      <section className="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
        <StatCard
          label="执行数"
          value={usage.data ? String(usage.data.summary.executions) : "-"}
          icon={Cpu}
          loading={usage.loading}
        />
        <StatCard
          label="总 Token"
          value={usage.data ? formatTokens(usage.data.summary.totalTokens) : "-"}
          hint={usage.data ? `输入 ${formatTokens(usage.data.summary.promptTokens)}` : undefined}
          icon={Coins}
          loading={usage.loading}
        />
        <StatCard
          label="输出 Token"
          value={usage.data ? formatTokens(usage.data.summary.completionTokens) : "-"}
          icon={Coins}
          loading={usage.loading}
        />
        <StatCard
          label="累计耗时"
          value={usage.data ? formatDuration(usage.data.summary.durationMs) : "-"}
          icon={Timer}
          loading={usage.loading}
        />
      </section>

      <Card className="border-border/70 bg-card/60">
        <CardHeader>
          <CardTitle className="text-base">按模型汇总</CardTitle>
          <CardDescription>不同 Runtime 的计费口径不同，汇总以服务端记录为准。</CardDescription>
        </CardHeader>
        <CardContent>
          {usage.data && usage.data.summary.byModel.length === 0 ? (
            <EmptyState icon={Coins} title="暂无用量记录" description="执行结束后，Runtime 上报的 token 计数会在这里出现。" />
          ) : null}
          {usage.data && usage.data.summary.byModel.length > 0 ? (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>模型</TableHead>
                  <TableHead>执行数</TableHead>
                  <TableHead>总 Token</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {usage.data.summary.byModel.map(item => (
                  <TableRow key={item.model}>
                    <TableCell className="font-mono text-xs">{item.model}</TableCell>
                    <TableCell className="text-xs tabular-nums">{item.executions}</TableCell>
                    <TableCell className="text-xs tabular-nums">{formatTokens(item.totalTokens)}</TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          ) : null}
        </CardContent>
      </Card>

      <Card className="border-border/70 bg-card/60">
        <CardHeader>
          <CardTitle className="text-base">执行明细</CardTitle>
        </CardHeader>
        <CardContent>
          {usage.data && usage.data.records.length === 0 ? (
            <EmptyState icon={Coins} title="没有记录" />
          ) : null}
          {usage.data && usage.data.records.length > 0 ? (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>执行 ID</TableHead>
                  <TableHead>模型</TableHead>
                  <TableHead>输入</TableHead>
                  <TableHead>输出</TableHead>
                  <TableHead>合计</TableHead>
                  <TableHead>耗时</TableHead>
                  <TableHead>结束时间</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {usage.data.records.map(record => (
                  <TableRow key={record.executionId}>
                    <TableCell className="font-mono text-xs">{record.executionId}</TableCell>
                    <TableCell className="text-xs">{record.model}</TableCell>
                    <TableCell className="text-xs tabular-nums">{formatTokens(record.promptTokens)}</TableCell>
                    <TableCell className="text-xs tabular-nums">{formatTokens(record.completionTokens)}</TableCell>
                    <TableCell className="text-xs tabular-nums">{formatTokens(record.totalTokens)}</TableCell>
                    <TableCell className="text-xs tabular-nums">{formatDuration(record.durationMs)}</TableCell>
                    <TableCell className="text-xs text-muted-foreground">
                      {formatTime(record.finishedAt)}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          ) : null}
        </CardContent>
      </Card>
    </div>
  );
}
