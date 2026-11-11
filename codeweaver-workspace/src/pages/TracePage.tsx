import { useState } from "react";
import { GitCommitHorizontal, Loader2, Search } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { EmptyState, ErrorNotice } from "@/components/common/EmptyState";
import { StatusBadge } from "@/components/common/StatusBadge";
import { api } from "@/lib/api";
import { formatTime } from "@/lib/format";
import type { CommitTrace } from "@/lib/types";

/**
 * Commit → agent run traceability (design 15).
 *
 * The lookup goes through the server: the UI only asks for a commit and renders the chain of records
 * (change record → context / prompt / policy snapshots → evaluations) that the server resolved.
 */
export function TracePage() {
  const [query, setQuery] = useState("");
  const [trace, setTrace] = useState<CommitTrace | undefined>(undefined);
  const [error, setError] = useState<unknown>(undefined);
  const [loading, setLoading] = useState(false);

  const lookup = async () => {
    const value = query.trim();
    if (value === "") return;
    setLoading(true);
    setError(undefined);
    try {
      setTrace(await api.traceCommit(value));
    } catch (caught) {
      setTrace(undefined);
      setError(caught);
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="space-y-4">
      <Card className="border-border/70 bg-card/60">
        <CardHeader>
          <CardTitle className="text-base">从 Git Commit 反查 Agent Run</CardTitle>
          <CardDescription>
            输入提交 SHA，服务端返回对应的变更记录以及上下文、提示词、策略快照与评估结果。
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-3">
          <div className="flex gap-2">
            <Input
              value={query}
              placeholder="例如 a1b2c3d"
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

          {!trace && !error && !loading ? (
            <EmptyState
              icon={GitCommitHorizontal}
              title="还没有查询结果"
              description="追溯链路完全由服务端记录生成，界面不推断提交与执行的关系。"
            />
          ) : null}
        </CardContent>
      </Card>

      {trace ? (
        <div className="grid gap-4 lg:grid-cols-2">
          <Card className="border-border/70 bg-card/60">
            <CardHeader>
              <CardTitle className="text-base">变更记录</CardTitle>
              <CardDescription className="font-mono">
                {trace.changeRecord.gitCommitSha ?? query.trim()}
              </CardDescription>
            </CardHeader>
            <CardContent>
              <pre className="max-h-96 overflow-auto rounded-lg border border-border/60 bg-background/60 p-3 text-xs whitespace-pre-wrap text-muted-foreground">
                {JSON.stringify(trace.changeRecord, null, 2)}
              </pre>
            </CardContent>
          </Card>

          <div className="space-y-4">
            <Card className="border-border/70 bg-card/60">
              <CardHeader>
                <CardTitle className="text-base">执行链路</CardTitle>
                <CardDescription className="font-mono">{trace.trace.executionId}</CardDescription>
              </CardHeader>
              <CardContent className="space-y-4">
                <SnapshotList
                  title="上下文快照"
                  items={trace.trace.contextSnapshots.map(item => ({
                    id: item.id,
                    hash: item.contextHash,
                    createdAt: item.createdAt,
                    extra: `${item.architectureVersion} · ${item.selectedFiles.length} 个文件`
                  }))}
                />
                <SnapshotList
                  title="提示词快照"
                  items={trace.trace.promptSnapshots.map(item => ({
                    id: item.id,
                    hash: item.renderedPromptHash,
                    createdAt: item.createdAt,
                    extra: `${item.templateName}@${item.templateVersion}`
                  }))}
                />
                <SnapshotList
                  title="策略快照"
                  items={trace.trace.policySnapshots.map(item => ({
                    id: item.id,
                    hash: item.policyVersion,
                    createdAt: item.createdAt,
                    extra: `授予 ${item.grantedCapabilities.length} 项能力`
                  }))}
                />
                <SnapshotList
                  title="评估结果"
                  items={trace.trace.evaluations.map(item => ({
                    id: item.executionId,
                    hash: item.summary,
                    createdAt: "",
                    extra: item.verdict,
                    status: item.verdict
                  }))}
                />
              </CardContent>
            </Card>
          </div>
        </div>
      ) : null}
    </div>
  );
}

function SnapshotList({
  title,
  items
}: {
  title: string;
  items: Array<{ id: string; hash: string; createdAt: string; extra: string; status?: string }>;
}) {
  return (
    <div>
      <p className="mb-2 text-xs font-medium text-muted-foreground">{title}</p>
      {items.length === 0 ? (
        <p className="text-xs text-muted-foreground">无记录</p>
      ) : (
        <ul className="space-y-2">
          {items.map(item => (
            <li
              key={`${title}-${item.id}`}
              className="flex items-start justify-between gap-3 rounded-lg border border-border/60 bg-background/40 px-3 py-2"
            >
              <div className="min-w-0">
                <p className="truncate font-mono text-xs">{item.id}</p>
                <p className="truncate text-[11px] text-muted-foreground">{item.extra}</p>
                <p className="truncate font-mono text-[10px] text-muted-foreground/70">{item.hash}</p>
              </div>
              <div className="flex shrink-0 items-center gap-2">
                {item.status ? <StatusBadge status={item.status} /> : null}
                {item.createdAt ? (
                  <span className="text-[10px] text-muted-foreground">{formatTime(item.createdAt)}</span>
                ) : null}
              </div>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
