import { useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { Loader2, Play } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle
} from "@/components/ui/sheet";
import { Textarea } from "@/components/ui/textarea";
import { Switch } from "@/components/ui/switch";
import { api } from "@/lib/api";
import { sha256Hex } from "@/lib/hash";
import { useApiData } from "@/hooks/useApiData";
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
  const [projectId, setProjectId] = useState("codeweaver");
  const [prompt, setPrompt] = useState("");
  const [worktreePath, setWorktreePath] = useState("");
  const [architectureVersion, setArchitectureVersion] = useState("v1");
  const [documents, setDocuments] = useState("");
  const [selectedFiles, setSelectedFiles] = useState("");
  // The tools CodeWeaver provides. The default is the whole set, because a task that cannot change
  // anything is not the product: the Pi adapter refuses a tool that is not in the allowed list
  // before it runs, so a default of "read only" would silently turn every console task into a
  // reading exercise.
  const [allowedTools, setAllowedTools] = useState(
    "read_file,request_diff,request_apply_diff,evaluate_changes"
  );
  const [evaluationChecks, setEvaluationChecks] = useState("");
  const [timeoutSeconds, setTimeoutSeconds] = useState("60");
  const [policyVersion, setPolicyVersion] = useState("v1");
  // The capabilities the policy grants. The Code Engine needs `modify_worktree` before it will
  // write, so a task that only grants reading cannot land a change either.
  const [grantedCapabilities, setGrantedCapabilities] = useState(
    "read_file,search_code,generate_diff,modify_worktree"
  );
  const [allowedPaths, setAllowedPaths] = useState("");
  const [expiresAt, setExpiresAt] = useState(() => new Date(Date.now() + 3600_000).toISOString());
  const [templateName, setTemplateName] = useState("default");
  const [templateVersion, setTemplateVersion] = useState("1.0.0");
  const [submitting, setSubmitting] = useState(false);
  const [advancedOpen, setAdvancedOpen] = useState(false);
  // The runtime that runs the task. `auto` lets the server pick a healthy one; naming a runtime
  // is honoured or refused, never quietly redirected (D-047).
  const [runtimeId, setRuntimeId] = useState("auto");
  // Both lists are read when the sheet opens, not when the app mounts: a runtime or a skill
  // registered after the page loaded would otherwise be missing from the picker.
  const runtimes = useApiData(() => api.listRuntimes(), [open]);
  /**
   * The default runtime is the one the project runs tasks on (D-042, D-060).
   *
   * "自动选择" stays the fallback for a stack without it, but as the *default* it hands the task
   * to whichever runtime the scheduler happens to pick first — and in the Pi stack that is the
   * mock Hermes, which answers "hello world" to everything and looks successful without having
   * done anything. The picker still offers both; the default just stops pretending.
   */
  useEffect(() => {
    const list = runtimes.data?.runtimes ?? [];
    if (!list.some(item => item.id === "pi-local")) return;
    setRuntimeId(current => (current === "auto" ? "pi-local" : current));
  }, [runtimes.data]);
  // The skills this task binds. Their bodies are injected into the prompt the runtime receives, so
  // a task without skills is not the same task as one with them.
  const [selectedSkills, setSelectedSkills] = useState<Array<{ skillId: string; version: string }>>([]);
  const [approvedSkills, setApprovedSkills] = useState<string[]>([]);
  const skills = useApiData(() => api.listSkills(), [open]);

  const skillKey = (skillId: string, version: string) => `${skillId}@${version}`;
  const isSkillSelected = (skillId: string, version: string) =>
    selectedSkills.some(item => item.skillId === skillId && item.version === version);
  const toggleSkill = (skillId: string, version: string) => {
    setSelectedSkills(current =>
      isSkillSelected(skillId, version)
        ? current.filter(item => !(item.skillId === skillId && item.version === version))
        : [...current, { skillId, version }]
    );
  };
  const toggleApproval = (skillId: string, version: string) => {
    const key = skillKey(skillId, version);
    setApprovedSkills(current =>
      current.includes(key) ? current.filter(item => item !== key) : [...current, key]
    );
  };

  const splitList = (raw: string) =>
    raw
      .split(/[\n,]/)
      .map(item => item.trim())
      .filter(item => item !== "");

  /**
   * The acceptance checks, one per line: a command and its arguments.
   *
   * The check runs in the worktree, so it sees the change the task made. Without a check the
   * completion gate has nothing to decide on and every run ends as "no evaluation result".
   */
  const parseChecks = (raw: string, worktree: string) =>
    splitList(raw)
      .map(line => line.split(/\s+/).filter(part => part !== ""))
      .filter(parts => parts.length > 0)
      .map((parts, index) => ({
        id: `check-${index + 1}`,
        name: parts.join(" "),
        command: parts[0]!,
        args: parts.slice(1),
        cwd: worktree
      }));

  const payload = useMemo<StartTaskInput | undefined>(() => {
    if (taskId.trim() === "" || prompt.trim() === "" || worktreePath.trim() === "") return undefined;
    return {
      projectId: projectId.trim() || "codeweaver",
      prompt,
      worktreePath: worktreePath.trim(),
      documents: splitList(documents),
      selectedFiles: splitList(selectedFiles),
      architectureVersion: architectureVersion.trim() || "v1",
      policy: {
        snapshotId: "pol_pending",
        projectId: projectId.trim() || "codeweaver",
        taskId: taskId.trim(),
        executionId: "pending",
        worktreeId: taskId.trim(),
        policyVersion: policyVersion.trim() || "v1",
        grantedCapabilities: splitList(grantedCapabilities),
        // A path prefix authorizes writes. The worktree the task was pointed at is the scope the
        // user already named, so an empty field means "the worktree" rather than "nothing" — with
        // an empty list the policy denies every write with "outside every authorized path".
        allowedPaths:
          splitList(allowedPaths).length > 0 ? splitList(allowedPaths) : [worktreePath.trim()],
        expiresAt
      },
      template: {
        name: templateName.trim() || "default",
        version: templateVersion.trim() || "1.0.0",
        contentHash: ""
      },
      modelConfig: {},
      timeoutSeconds: Number(timeoutSeconds) || 60,
      allowedTools: splitList(allowedTools),
      runtimeId,
      ...(parseChecks(evaluationChecks, worktreePath.trim()).length > 0
        ? { evaluationChecks: parseChecks(evaluationChecks, worktreePath.trim()) }
        : {}),
      ...(selectedSkills.length > 0
        ? {
            skills: selectedSkills.map(item => ({ skillId: item.skillId, version: item.version })),
            ...(approvedSkills.length > 0 ? { approvedSkills } : {})
          }
        : {})
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
    allowedTools,
    runtimeId,
    evaluationChecks,
    selectedSkills,
    approvedSkills
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
              placeholder="D:\\github\\CodeWeaver"
              onChange={event => setWorktreePath(event.target.value)}
            />
          </div>

          <div className="space-y-2">
            <Label htmlFor="runtime">运行时</Label>
            <Select value={runtimeId} onValueChange={setRuntimeId}>
              <SelectTrigger id="runtime">
                <SelectValue placeholder="自动选择" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="auto">自动选择（由服务端调度）</SelectItem>
                {runtimes.data?.runtimes.map(item => (
                  <SelectItem key={item.id} value={item.id}>
                    {item.id} · {item.runtimeType}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <p className="text-xs text-muted-foreground">
              运行时由服务端注册；点名的运行时要么就用它，要么被拒绝，不会被悄悄换成别的。
            </p>
          </div>

          <div className="space-y-2">
            <Label>绑定技能</Label>
            {skills.data && skills.data.skills.length === 0 ? (
              <p className="text-xs text-muted-foreground">
                技能库为空。先在「技能管理」注册一个技能，它正文才会被注入到执行的 prompt 里。
              </p>
            ) : null}
            <div className="space-y-2">
              {skills.data?.skills.map(skill => {
                const key = skillKey(skill.skillId, skill.version);
                const selected = isSkillSelected(skill.skillId, skill.version);
                const needsApproval = skill.approvalPolicy === "require_approval";
                return (
                  <div
                    key={key}
                    className="flex items-start justify-between gap-3 rounded-md border border-border/70 px-3 py-2"
                  >
                    <button
                      type="button"
                      onClick={() => toggleSkill(skill.skillId, skill.version)}
                      className="min-w-0 flex-1 text-left"
                    >
                      <p className="truncate text-sm">
                        {skill.name}
                        {selected ? " · 已绑定" : ""}
                      </p>
                      <p className="truncate font-mono text-xs text-muted-foreground">{key}</p>
                      <p className="truncate text-xs text-muted-foreground">
                        {skill.permissions.join(", ") || "无额外权限"} · {skill.riskLevel}
                      </p>
                    </button>
                    {needsApproval ? (
                      <div className="flex items-center gap-2 pt-1">
                        <Label htmlFor={`approve-${key}`} className="text-xs text-muted-foreground">
                          我已审批
                        </Label>
                        <Switch
                          id={`approve-${key}`}
                          checked={approvedSkills.includes(key)}
                          onCheckedChange={() => toggleApproval(skill.skillId, skill.version)}
                        />
                      </div>
                    ) : null}
                  </div>
                );
              })}
            </div>
            <p className="text-xs text-muted-foreground">
              技能正文会在执行开始时随 prompt 一起交给运行时；服务端会按策略核对权限与运行环境，未通过的技能会直接拒绝这次执行。
            </p>
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

          <div className="space-y-2">
            <Label htmlFor="checks">验收检查（每行一条：命令 + 参数）</Label>
            <Textarea
              id="checks"
              rows={3}
              value={evaluationChecks}
              placeholder={"node check.mjs"}
              onChange={event => setEvaluationChecks(event.target.value)}
            />
            <p className="text-xs text-muted-foreground">
              检查在工作区里独立运行，决定「完成门禁」是否放行；没有检查时运行会以「无评估结果」收尾。
            </p>
            <p className="text-xs text-muted-foreground">
              危险命令（删除、提权、发布、管道进 shell 等）默认被拒绝：只有针对
              <span className="font-mono">这一条命令</span>
              的审批才能放行，审批不能挪用到别的命令。
            </p>
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
                <Label htmlFor="allowed-paths">允许写入的路径前缀（留空则用工作区路径）</Label>
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
