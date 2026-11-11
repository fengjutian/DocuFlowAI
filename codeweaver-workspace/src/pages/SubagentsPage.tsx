import { useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { GitBranch, Layers, RefreshCw, Search, Waypoints } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Badge } from "@/components/ui/badge";
import { EmptyState, ErrorNotice } from "@/components/common/EmptyState";
import { StatusBadge } from "@/components/common/StatusBadge";
import { subagentApi, type SubagentTree } from "@/lib/subagent-api";
import { shortId } from "@/lib/format";
import { useApiData } from "@/hooks/useApiData";
import { useExecutionStream } from "@/state/execution-stream";

/** What the console knows about one child execution. */
interface ChildRow {
  executionId: string;
  status?: string;
  runtimeId?: string;
  eventCount?: number;
  live?: boolean;
  /** Set when the server could not describe this child; the row is still shown. */
  error?: string;
}

/**
 * Subagent delegation.
 *
 * The server owns the lineage: a parent execution knows its children, and every execution knows the
 * chain it belongs to. Nothing here is inferred from event order — the page asks the server and
 * shows the answer, including the case where the server has no delegation to show.
 *
 * The execution list comes from the live event stream, because the lab server exposes no "list
 * executions" endpoint; a direct-by-id box covers the executions that started before this console
 * was opened.
 */
export function SubagentsPage() {
  const navigate = useNavigate();
  const { executions, streamStatus } = useExecutionStream();
  const [selectedId, setSelectedId] = useState<string>("");
  const [manualId, setManualId] = useState<string>("");

  const tree = useApiData<SubagentTree>(
    () => subagentApi.listSubagents(selectedId),
    [selectedId],
    { enabled: selectedId !== "" }
  );

  const childIds = useMemo(() => tree.data?.children ?? [], [tree.data]);
  const children = useApiData<ChildRow[]>(
    async () =>
      Promise.all(
        childIds.map(async executionId => {
          try {
            const detail = await subagentApi.getExecution(executionId);
            return {
              executionId,
              status: detail.status,
              runtimeId: detail.runtimeId,
              eventCount: detail.checkpoint?.eventCount,
              live: detail.live
            };
          } catch (error) {
            return {
              executionId,
              error: error instanceof Error ? error.message : String(error)
            };
          }
        })
      ),
    [childIds.join(",")],
    { enabled: selectedId !== "" && childIds.length > 0 }
  );

  const lookup = (executionId: string) => {
    const trimmed = executionId.trim();
    if (trimmed === "") return;
    setSelectedId(trimmed);
    toast.success(`已切换到执行 ${shortId(trimmed, 12)}`);
  };

  const refresh = () => {
    tree.refresh();
    children.refresh();
    toast.success("已刷新子代理血缘");
  };

  const lineage = tree.data?.lineage ?? [];
  const rootId = tree.data?.rootExecutionId;
  // A manually typed id is not in the event stream, so the select falls back to its placeholder
  // instead of pretending it knows an execution it has never seen.
  const knownExecution = executions.some(item => item.executionId === selectedId);

  return (
    <div className="space-y-4">
      <Card className="border-border/70 bg-card/60">
        <CardHeader className="flex-row items-start justify-between space-y-0">
          <div>
            <CardTitle className="text-base">子代理委派</CardTitle>
            <CardDescription>
              选一个父执行，这里显示它的子执行与血缘（根执行、父子关系）。
              数据来自服务端记录（{streamStatus === "open" ? "事件流已连接" : streamStatus}），不是控制台自己推断的。
            </CardDescription>
          </div>
          <Button size="sm" variant="outline" onClick={refresh} disabled={selectedId === ""}>
            <RefreshCw className="size-4" />
            刷新
          </Button>
        </CardHeader>
        <CardContent className="space-y-4">
          <div className="flex flex-col gap-2 sm:flex-row">
            <Select value={knownExecution ? selectedId : ""} onValueChange={value => lookup(value)}>
              <SelectTrigger className="w-full sm:w-96">
                <SelectValue placeholder="选择一个父执行" />
              </SelectTrigger>
              <SelectContent>
                {executions.map(item => (
                  <SelectItem key={item.executionId} value={item.executionId}>
                    {item.taskId ?? shortId(item.executionId, 12)} · {item.status}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <div className="flex flex-1 gap-2">
              <div className="relative flex-1">
                <Search className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
                <Input
                  value={manualId}
                  placeholder="按执行 ID 直接查看"
                  className="pl-9"
                  onChange={event => setManualId(event.target.value)}
                  onKeyDown={event => {
                    if (event.key === "Enter") lookup(manualId);
                  }}
                />
              </div>
              <Button variant="outline" disabled={manualId.trim() === ""} onClick={() => lookup(manualId)}>
                查看
              </Button>
            </div>
          </div>

          {tree.error ? <ErrorNotice error={tree.error} onRetry={tree.refresh} /> : null}
          {tree.loading ? <p className="text-sm text-muted-foreground">读取血缘中…</p> : null}
        </CardContent>
      </Card>

      <Card className="border-border/70 bg-card/60">
        <CardHeader>
          <CardTitle className="flex items-center gap-2 text-base">
            <Layers className="size-4 text-violet-600 dark:text-violet-400" />子执行
          </CardTitle>
          <CardDescription>
            {selectedId === ""
              ? "先在上方选择一个父执行。"
              : `父执行 ${shortId(selectedId, 16)} 的直接子执行，由服务端记录。`}
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-3">
          {selectedId === "" ? (
            <EmptyState
              icon={Waypoints}
              title="还没有选择父执行"
              description="子代理委派把一次执行拆成父子两级；选中父执行后，这里会列出它派生的子执行。"
            />
          ) : children.loading ? (
            <p className="text-sm text-muted-foreground">读取子执行中…</p>
          ) : children.error ? (
            <ErrorNotice error={children.error} onRetry={children.refresh} />
          ) : childIds.length === 0 && !tree.loading ? (
            <EmptyState
              icon={Layers}
              title="该执行还没有子执行"
              description="当父执行通过委派接口派生子执行后，服务器会在这里记录父子关系与血缘。"
            />
          ) : (
            <ScrollArea className="max-h-[420px]">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>子执行 ID</TableHead>
                    <TableHead>状态</TableHead>
                    <TableHead>运行时</TableHead>
                    <TableHead>事件</TableHead>
                    <TableHead>进程内会话</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {(children.data ?? []).map(child => (
                    <TableRow
                      key={child.executionId}
                      className="cursor-pointer"
                      onClick={() => navigate(`/executions/${child.executionId}`)}
                    >
                      <TableCell className="font-mono text-xs">{child.executionId}</TableCell>
                      <TableCell>
                        {child.error ? (
                          <span className="text-xs text-destructive">读取失败</span>
                        ) : (
                          <StatusBadge status={child.status} />
                        )}
                      </TableCell>
                      <TableCell className="text-xs text-muted-foreground">
                        {child.runtimeId ?? "-"}
                      </TableCell>
                      <TableCell className="text-xs tabular-nums">{child.eventCount ?? "-"}</TableCell>
                      <TableCell className="text-xs text-muted-foreground">
                        {child.live ? "活跃" : "已结束"}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </ScrollArea>
          )}
        </CardContent>
      </Card>

      <Card className="border-border/70 bg-card/60">
        <CardHeader>
          <CardTitle className="flex items-center gap-2 text-base">
            <GitBranch className="size-4 text-sky-600 dark:text-sky-400" />血缘
          </CardTitle>
          <CardDescription>
            从当前执行一路向上到根执行。服务端记录的血缘链，任何一环缺失都会在这里显出来。
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-3">
          {selectedId === "" ? (
            <EmptyState icon={GitBranch} title="还没有选择执行" description="选中一个执行后，这里显示它的血缘链。" />
          ) : lineage.length === 0 && !tree.loading ? (
            <EmptyState
              icon={GitBranch}
              title="服务端没有记录血缘"
              description="该执行没有父执行，也没有子执行；血缘链只有它自己。"
            />
          ) : (
            <ol className="space-y-2">
              {lineage.map((executionId, index) => {
                const isRoot = executionId === rootId;
                const isCurrent = executionId === selectedId;
                return (
                  <li
                    key={executionId}
                    className="flex items-center gap-3 rounded-xl border border-border/60 bg-background/35 p-3"
                  >
                    <span className="flex size-8 shrink-0 items-center justify-center rounded-lg border border-border/60 bg-background/70 text-muted-foreground">
                      <GitBranch className="size-4" />
                    </span>
                    <div className="min-w-0 flex-1">
                      <p className="truncate font-mono text-xs">{executionId}</p>
                      <p className="mt-1 text-xs text-muted-foreground">
                        {index === 0 ? "当前执行" : `第 ${index + 1} 级子执行`}
                      </p>
                    </div>
                    {isRoot ? (
                      <Badge variant="outline" className="border-emerald-500/40 bg-emerald-500/15 text-emerald-700 dark:text-emerald-300">
                        根执行
                      </Badge>
                    ) : null}
                    {isCurrent ? (
                      <Badge variant="outline" className="border-sky-500/40 bg-sky-500/15 text-sky-700 dark:text-sky-300">
                        当前
                      </Badge>
                    ) : null}
                  </li>
                );
              })}
            </ol>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
