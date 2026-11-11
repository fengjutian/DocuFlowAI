import { useState } from "react";
import { GitCommitHorizontal, Link2, Loader2, Search } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Separator } from "@/components/ui/separator";
import { EmptyState, ErrorNotice } from "@/components/common/EmptyState";
import { StatusBadge } from "@/components/common/StatusBadge";
import { api } from "@/lib/api";
import type { ChangeRecord, ExecutionTrace } from "@/lib/types";

/**
 * End-to-end traceability (design 15).
 *
 * The chain is Task → Run → Tool → Change → Commit → Test, and the page shows it as a chain: one
 * row per link, each saying what it holds. A link with nothing in it is shown as missing rather
 * than dropped — a chain with a hole in it is information the reader needs, and hiding it would
 * make a broken chain look like a short one.
 *
 * The lookup goes through the server: the console asks for a commit, an execution or a task and
 * renders the records the server resolved. It never rebuilds the chain from the event stream.
 */

type LookupMode = "commit" | "execution" | "task";

interface TraceView {
  heading: string;
  traces: ExecutionTrace[];
  changeRecord?: ChangeRecord;
}

export function TracePage() {
  const [mode, setMode] = useState<LookupMode>("commit");
  const [query, setQuery] = useState("");
  const [view, setView] = useState<TraceView | undefined>(undefined);
  const [error, setError] = useState<unknown>(undefined);
  const [loading, setLoading] = useState(false);

  const lookup = async () => {
    const value = query.trim();
    if (value === "") return;
    setLoading(true);
    setError(undefined);
    try {
      if (mode === "commit") {
        const found = await api.traceCommit(value);
        setView({
          heading: `提交 ${found.changeRecord.gitCommitSha ?? value}`,
          traces: [found.trace],
          changeRecord: found.changeRecord
        });
        return;
      }
      if (mode === "execution") {
        const found = await api.traceExecution(value);
        setView({ heading: `执行 ${found.executionId}`, traces: [found] });
        return;
      }
      const found = await api.traceTask(value);
      setView({
        heading:
          found.executions.length > 0
            ? `任务 ${found.taskId}（${found.executions.length} 个执行）`
            : `任务 ${found.taskId}`,
        traces: found.executions
      });
    } catch (caught) {
      setView(undefined);
      setError(caught);
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="space-y-4">
      <Card className="border-border/70 bg-card/60">
        <CardHeader>
          <CardTitle className="text-base">端到端追溯</CardTitle>
          <CardDescription>
            Task → Run → Tool → Change → Commit → Test。每一环都取自服务端记录，缺失的环节会明确显示为缺失。
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-3">
          <div className="flex flex-wrap gap-2">
            <Select value={mode} onValueChange={value => setMode(value as LookupMode)}>
              <SelectTrigger id="trace-mode" className="w-44">
                <SelectValue placeholder="选择查询方式" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="commit">按 Git Commit</SelectItem>
                <SelectItem value="execution">按执行 ID</SelectItem>
                <SelectItem value="task">按任务 ID</SelectItem>
              </SelectContent>
            </Select>
            <Input
              value={query}
              placeholder={mode === "commit" ? "例如 a1b2c3d" : mode === "execution" ? "exec_..." : "TASK-001"}
              className="font-mono"
              onChange={event => setQuery(event.target.value)}
              onKeyDown={event => {
                if (event.key === "Enter") void lookup();
              }}
            />
            <Button onClick={() => void lookup()} disabled={loading || query.trim() === ""}>
              {loading ? <Loader2 className="size-4 animate-spin" /> : <Search className="size-4" />}
              查询
            </Button>
          </div>

          {error ? <ErrorNotice error={error} /> : null}

          {!view && !error && !loading ? (
            <EmptyState
              icon={GitCommitHorizontal}
              title="还没有查询结果"
              description="追溯链路完全由服务端记录生成，界面不推断提交与执行的关系。"
            />
          ) : null}
        </CardContent>
      </Card>

      {view
        ? view.traces.length > 0
          ? view.traces.map(trace => (
              <ChainCard
                key={trace.executionId}
                heading={view.heading}
                trace={trace}
                changeRecord={view.changeRecord}
              />
            ))
          : (
              <EmptyState
                icon={Search}
                title="没有执行记录"
                description={`服务端没有记录 ${view.heading} 的任何执行。任务可能从未启动，或者它的执行在写下第一条记录之前就结束了。`}
              />
            )
        : null}
    </div>
  );
}

/**
 * One execution's chain.
 *
 * Each link is a row that says what it holds and whether it holds anything. The tool row lists
 * the calls with the decision that gated them, because "the runtime asked" is only half of what a
 * reader needs.
 */
function ChainCard({
  heading,
  trace,
  changeRecord
}: {
  heading: string;
  trace: ExecutionTrace;
  changeRecord?: ChangeRecord;
}) {
  const links = [
    {
      name: "任务",
      ok: trace.taskIds.length > 0,
      detail: trace.taskIds.length > 0 ? trace.taskIds.join(", ") : "无记录"
    },
    {
      name: "运行",
      ok: Boolean(trace.runtimeId),
      detail: [
        trace.runtimeId ?? "未知运行时",
        trace.executionId,
        trace.externalRunId ?? ""
      ]
        .filter(item => item !== "")
        .join(" · ")
    },
    {
      name: "工具",
      ok: trace.toolCalls.length > 0,
      detail:
        trace.toolCalls.length > 0
          ? trace.toolCalls.map(call => call.toolName).join(" > ")
          : "无记录"
    },
    {
      name: "变更",
      ok: trace.changeRecords.length > 0,
      detail:
        trace.changeRecords.length > 0
          ? trace.changeRecords.map(record => record.id).join(", ")
          : "无记录"
    },
    {
      name: "提交",
      ok: trace.gitCommits.length > 0,
      detail: trace.gitCommits.length > 0 ? trace.gitCommits.join(", ") : "无记录"
    },
    {
      name: "测试",
      ok: trace.evaluations.some(item => item.verdict === "passed"),
      detail:
        trace.evaluations.length > 0
          ? trace.evaluations.map(item => item.verdict).join(", ")
          : "无记录"
    }
  ];

  return (
    <Card className="border-border/70 bg-card/60">
      <CardHeader>
        <CardTitle className="flex items-center gap-2 text-base">
          <Link2 className="size-4 text-muted-foreground" />
          {heading}
        </CardTitle>
        <CardDescription className="font-mono">{trace.executionId}</CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        <ol className="space-y-2">
          {links.map(link => (
            <li
              key={link.name}
              className="flex flex-wrap items-center justify-between gap-3 rounded-lg border border-border/60 bg-background/40 px-3 py-2"
            >
              <span className="text-xs font-medium text-muted-foreground">{link.name}</span>
              <span className="min-w-0 flex-1 truncate font-mono text-xs" title={link.detail}>
                {link.detail}
              </span>
              <StatusBadge status={link.ok ? "passed" : "not_run"} />
            </li>
          ))}
        </ol>

        {trace.toolCalls.length > 0 ? (
          <>
            <Separator className="bg-border/60" />
            <div className="space-y-2">
              <p className="text-xs font-medium text-muted-foreground">工具调用与权限决策</p>
              <ul className="space-y-1.5">
                {trace.toolCalls.map(call => (
                  <li
                    key={call.toolCallId}
                    className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-border/50 bg-background/30 px-2.5 py-1.5 text-xs"
                  >
                    <span className="font-mono">{call.toolName}</span>
                    <span className="text-muted-foreground">#{call.sequence}</span>
                    <StatusBadge status={call.status} />
                    {call.permission ? (
                      <span className="text-muted-foreground">
                        决策 {call.permission.decision}
                        {call.permission.reason ? ` · ${call.permission.reason}` : ""}
                      </span>
                    ) : (
                      <span className="text-muted-foreground">未经过策略门</span>
                    )}
                  </li>
                ))}
              </ul>
            </div>
          </>
        ) : null}

        {changeRecord ? (
          <>
            <Separator className="bg-border/60" />
            <pre className="max-h-72 overflow-auto rounded-lg border border-border/60 bg-background/60 p-3 text-xs whitespace-pre-wrap text-muted-foreground">
              {JSON.stringify(changeRecord, null, 2)}
            </pre>
          </>
        ) : null}

        {trace.contextSnapshots.length > 0 || trace.evaluations.length > 0 ? (
          <>
            <Separator className="bg-border/60" />
            <dl className="grid grid-cols-2 gap-x-3 gap-y-2 text-xs">
              <Field label="上下文快照" value={trace.contextSnapshots.map(item => item.id).join(", ") || "-"} />
              <Field label="提示词快照" value={trace.promptSnapshots.map(item => item.id).join(", ") || "-"} />
              <Field label="策略快照" value={trace.policySnapshots.map(item => item.id).join(", ") || "-"} />
              <Field
                label="评估结论"
                value={trace.evaluations.map(item => item.verdict).join(", ") || "-"}
              />
            </dl>
          </>
        ) : null}
      </CardContent>
    </Card>
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
