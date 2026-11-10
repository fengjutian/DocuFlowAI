import { useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { Activity, ArrowRight, Play, Search } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { EmptyState } from "@/components/common/EmptyState";
import { StatusBadge } from "@/components/common/StatusBadge";
import { relativeTime } from "@/lib/format";
import { useExecutionStream } from "@/state/execution-stream";

/**
 * Execution list.
 *
 * The lab server exposes no "list executions" endpoint, so the list is built from the live event
 * stream. A direct-by-id box covers the case where an execution started before this client was
 * opened and therefore produced no live events here.
 */
export function ExecutionsPage({ onNewTask }: { onNewTask: () => void }) {
  const navigate = useNavigate();
  const { executions, streamStatus } = useExecutionStream();
  const [filter, setFilter] = useState("");
  const [directId, setDirectId] = useState("");

  const filtered = useMemo(() => {
    const keyword = filter.trim().toLowerCase();
    if (keyword === "") return executions;
    return executions.filter(
      item =>
        item.executionId.toLowerCase().includes(keyword) ||
        (item.taskId ?? "").toLowerCase().includes(keyword) ||
        item.projectId.toLowerCase().includes(keyword)
    );
  }, [executions, filter]);

  return (
    <div className="space-y-4">
      <Card className="border-border/70 bg-card/60">
        <CardHeader className="flex-row items-start justify-between space-y-0">
          <div>
            <CardTitle className="text-base">执行</CardTitle>
            <CardDescription>
              数据来自实时事件流（{streamStatus === "open" ? "已连接" : streamStatus}）；事件不会因为刷新页面而丢失，
              重新连接后按 sequence 补齐。
            </CardDescription>
          </div>
          <Button size="sm" onClick={onNewTask}>
            <Play className="size-4" />
            新建执行
          </Button>
        </CardHeader>
        <CardContent className="space-y-4">
          <div className="flex flex-col gap-2 sm:flex-row">
            <div className="relative flex-1">
              <Search className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
              <Input
                value={filter}
                placeholder="按执行 ID、任务 ID 或项目过滤"
                className="pl-9"
                onChange={event => setFilter(event.target.value)}
              />
            </div>
            <div className="flex gap-2">
              <Input
                value={directId}
                placeholder="按执行 ID 直接打开"
                className="sm:w-64"
                onChange={event => setDirectId(event.target.value)}
                onKeyDown={event => {
                  if (event.key === "Enter" && directId.trim() !== "") {
                    navigate(`/executions/${directId.trim()}`);
                  }
                }}
              />
              <Button
                variant="outline"
                disabled={directId.trim() === ""}
                onClick={() => navigate(`/executions/${directId.trim()}`)}
              >
                <ArrowRight className="size-4" />
                打开
              </Button>
            </div>
          </div>

          {filtered.length === 0 ? (
            <EmptyState
              icon={Activity}
              title={executions.length === 0 ? "还没有执行事件" : "没有匹配的执行"}
              description={
                executions.length === 0
                  ? "启动一次执行后，这里会实时列出状态、事件数与最近活动时间。"
                  : "换个关键字，或用上方“按执行 ID 直接打开”进入指定执行。"
              }
            />
          ) : (
            <ScrollArea className="max-h-[520px]">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>执行 ID</TableHead>
                    <TableHead>任务</TableHead>
                    <TableHead>状态</TableHead>
                    <TableHead>事件</TableHead>
                    <TableHead>最近活动</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {filtered.map(item => (
                    <TableRow
                      key={item.executionId}
                      className="cursor-pointer"
                      onClick={() => navigate(`/executions/${item.executionId}`)}
                    >
                      <TableCell className="font-mono text-xs">{item.executionId}</TableCell>
                      <TableCell className="text-xs text-muted-foreground">
                        {item.taskId ?? "-"}
                      </TableCell>
                      <TableCell>
                        <StatusBadge status={item.status} />
                      </TableCell>
                      <TableCell className="text-xs tabular-nums">{item.eventCount}</TableCell>
                      <TableCell className="text-xs text-muted-foreground">
                        {relativeTime(item.lastTimestamp)}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </ScrollArea>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
