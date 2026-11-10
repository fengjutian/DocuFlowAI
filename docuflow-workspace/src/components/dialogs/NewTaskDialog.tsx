import { useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { Loader2, Play } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle
} from "@/components/ui/sheet";
import { Textarea } from "@/components/ui/textarea";
import { api } from "@/lib/api";
import { sha256Hex } from "@/lib/hash";
import type { StartTaskInput } from "@/lib/types";

/**
 * Starts an execution through `POST /api/v1/tasks/{taskId}/executions`.
 *
 * The form follows the request contract exactly: documents and selected files are resolved inside the
 * worktree and hashed by the server, so they must be real paths relative to `worktreePath`. The
 * template content hash is computed here instead of being typed, because the server validates it as
 * a SHA-256 digest.
 */
export function NewTaskDialog({
  open,
  onOpenChange,
  defaultTaskId
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  defaultTaskId?: string;
}) {
  const navigate = useNavigate();
  const [taskId, setTaskId] = useState(defaultTaskId ?? "");
  const [projectId, setProjectId] = useState("docuflow");
  const [prompt, setPrompt] = useState("");
  const [worktreePath, setWorktreePath] = useState("");
  const [architectureVersion, setArchitectureVersion] = useState("v1");
  const [documents, setDocuments] = useState("");
  const [selectedFiles, setSelectedFiles] = useState("");
  const [allowedTools, setAllowedTools] = useState("read_file,search_code");
  const [timeoutSeconds, setTimeoutSeconds] = useState("60");
  const [policyVersion, setPolicyVersion] = useState("v1");
  const [grantedCapabilities, setGrantedCapabilities] = useState("read_file,search_code");
  const [allowedPaths, setAllowedPaths] = useState("");
  const [expiresAt, setExpiresAt] = useState(() => new Date(Date.now() + 3600_000).toISOString());
  const [templateName, setTemplateName] = useState("default");
  const [templateVersion, setTemplateVersion] = useState("1.0.0");
  const [submitting, setSubmitting] = useState(false);
  const [advancedOpen, setAdvancedOpen] = useState(false);

  const splitList = (raw: string) =>
    raw
      .split(/[\n,]/)
      .map(item => item.trim())
      .filter(item => item !== "");

  const payload = useMemo<StartTaskInput | undefined>(() => {
    if (taskId.trim() === "" || prompt.trim() === "" || worktreePath.trim() === "") return undefined;
    return {
      projectId: projectId.trim() || "docuflow",
      prompt,
      worktreePath: worktreePath.trim(),
      documents: splitList(documents),
      selectedFiles: splitList(selectedFiles),
      architectureVersion: architectureVersion.trim() || "v1",
      policy: {
        snapshotId: "pol_pending",
        projectId: projectId.trim() || "docuflow",
        taskId: taskId.trim(),
        executionId: "pending",
        worktreeId: taskId.trim(),
        policyVersion: policyVersion.trim() || "v1",
        grantedCapabilities: splitList(grantedCapabilities),
        allowedPaths: splitList(allowedPaths),
        expiresAt
      },
      template: {
        name: templateName.trim() || "default",
        version: templateVersion.trim() || "1.0.0",
        contentHash: ""
      },
      modelConfig: {},
      timeoutSeconds: Number(timeoutSeconds) || 60,
      allowedTools: splitList(allowedTools)
    };
  }, [    taskId,
    projectId,
    prompt,
    worktreePath,
    documents,
    selectedFiles,
    architectureVersion,
    policyVersion,
    grantedCapabilities,
    allowedPaths,
    expiresAt,
    templateName,
    templateVersion,
    timeoutSeconds,
    allowedTools
  ]);

  const submit = async () => {
    if (!payload) {
      toast.error("请填写任务 ID、提示词与工作区路径");
      return;
    }
    setSubmitting(true);
    try {
      const contentHash = await sha256Hex(`${payload.template.name}:${payload.template.version}:${payload.prompt}`);
      const result = await api.startTask(taskId.trim(), {
        ...payload,
        template: { ...payload.template, contentHash }
      });
      toast.success(`执行已启动：${result.execution.executionId}`);
      onOpenChange(false);
      navigate(`/executions/${result.execution.executionId}`);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent className="flex w-full flex-col gap-0 sm:max-w-xl">
        <SheetHeader>
          <SheetTitle>新建执行</SheetTitle>
          <SheetDescription>
            提交到 POST /api/v1/tasks/{taskId}/executions。文档与文件路径相对于工作区，且必须真实存在。
          </SheetDescription>
        </SheetHeader>

        <div className="grid flex-1 gap-4 overflow-y-auto px-4 py-4">
          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-2">
              <Label htmlFor="task-id">任务 ID</Label>
              <Input
                id="task-id"
                value={taskId}
                placeholder="TASK-001"
                onChange={event => setTaskId(event.target.value)}
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="project-id">项目 ID</Label>
              <Input id="project-id" value={projectId} onChange={event => setProjectId(event.target.value)} />
            </div>
          </div>

          <div className="space-y-2">
            <Label htmlFor="prompt">提示词</Label>
            <Textarea
              id="prompt"
              rows={4}
              value={prompt}
              placeholder="交给 Agent 的任务描述"
              onChange={event => setPrompt(event.target.value)}
            />
          </div>

          <div className="space-y-2">
            <Label htmlFor="worktree">工作区路径（git 仓库 / worktree）</Label>
            <Input
              id="worktree"
              value={worktreePath}
              placeholder="D:\\github\\DocuFlowAI"
              onChange={event => setWorktreePath(event.target.value)}
            />
          </div>

          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-2">
              <Label htmlFor="documents">文档路径（每行一个）</Label>
              <Textarea
                id="documents"
                rows={3}
                value={documents}
                placeholder={"doc/需求.md"}
                onChange={event => setDocuments(event.target.value)}
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="selected-files">选中文件（每行一个）</Label>
              <Textarea
                id="selected-files"
                rows={3}
                value={selectedFiles}
                placeholder={"src/main.ts"}
                onChange={event => setSelectedFiles(event.target.value)}
              />
            </div>
          </div>

          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-2">
              <Label htmlFor="arch">架构版本</Label>
              <Input
                id="arch"
                value={architectureVersion}
                onChange={event => setArchitectureVersion(event.target.value)}
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="timeout">超时（秒）</Label>
              <Input
                id="timeout"
                type="number"
                value={timeoutSeconds}
                onChange={event => setTimeoutSeconds(event.target.value)}
              />
            </div>
          </div>

          <Collapsible open={advancedOpen} onOpenChange={setAdvancedOpen}>
            <CollapsibleTrigger className="text-sm text-muted-foreground hover:text-foreground">
              策略快照与模板（高级）
            </CollapsibleTrigger>
            <CollapsibleContent className="mt-3 space-y-3">
              <div className="grid grid-cols-2 gap-3">
                <div className="space-y-2">
                  <Label htmlFor="policy-version">策略版本</Label>
                  <Input
                    id="policy-version"
                    value={policyVersion}
                    onChange={event => setPolicyVersion(event.target.value)}
                  />
                </div>
                <div className="space-y-2">
                  <Label htmlFor="expires-at">策略过期时间</Label>
                  <Input
                    id="expires-at"
                    type="datetime-local"
                    value={expiresAt.slice(0, 16)}
                    onChange={event => setExpiresAt(new Date(event.target.value).toISOString())}
                  />
                </div>
              </div>
              <div className="space-y-2">
                <Label htmlFor="granted">授予能力（逗号分隔）</Label>
                <Input
                  id="granted"
                  value={grantedCapabilities}
                  onChange={event => setGrantedCapabilities(event.target.value)}
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="allowed-paths">允许写入的路径前缀</Label>
                <Input
                  id="allowed-paths"
                  value={allowedPaths}
                  onChange={event => setAllowedPaths(event.target.value)}
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="tools">允许的工具（逗号分隔）</Label>
                <Input
                  id="tools"
                  value={allowedTools}
                  onChange={event => setAllowedTools(event.target.value)}
                />
              </div>
              <div className="grid grid-cols-2 gap-3">
                <div className="space-y-2">
                  <Label htmlFor="template-name">模板名称</Label>
                  <Input
                    id="template-name"
                    value={templateName}
                    onChange={event => setTemplateName(event.target.value)}
                  />
                </div>
                <div className="space-y-2">
                  <Label htmlFor="template-version">模板版本</Label>
                  <Input
                    id="template-version"
                    value={templateVersion}
                    onChange={event => setTemplateVersion(event.target.value)}
                  />
                </div>
              </div>
              <p className="text-xs text-muted-foreground">
                模板内容哈希由客户端按 SHA-256 计算，服务端会校验其格式。
              </p>
            </CollapsibleContent>
          </Collapsible>
        </div>

        <SheetFooter className="shrink-0">
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            取消
          </Button>
          <Button onClick={() => void submit()} disabled={submitting}>
            {submitting ? <Loader2 className="size-4 animate-spin" /> : <Play className="size-4" />}
            启动执行
          </Button>
        </SheetFooter>
      </SheetContent>
    </Sheet>
  );
}
