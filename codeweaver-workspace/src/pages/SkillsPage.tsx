import { useState } from "react";
import { Loader2, Plus, Puzzle, ShieldAlert, TriangleAlert } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Textarea } from "@/components/ui/textarea";
import { Badge } from "@/components/ui/badge";
import { EmptyState, ErrorNotice } from "@/components/common/EmptyState";
import { api } from "@/lib/api";
import { sha256Hex } from "@/lib/hash";
import { formatTime } from "@/lib/format";
import { useApiData } from "@/hooks/useApiData";
import type { RiskLevel, SkillKind } from "@/lib/types";

const RISK_TONE: Record<RiskLevel, string> = {
  low: "border-emerald-500/40 bg-emerald-500/15 text-emerald-300",
  medium: "border-amber-500/40 bg-amber-500/15 text-amber-300",
  high: "border-red-500/40 bg-red-500/15 text-red-300"
};

const KNOWN_PERMISSIONS = [
  "read_file",
  "search_code",
  "modify_worktree",
  "delete_file",
  "install_dependency",
  "run_command",
  "network_access",
  "git_merge"
];

/**
 * Skill registry.
 *
 * A skill is not trusted because it is Markdown: the registry only accepts a manifest that declares
 * its permissions, runtimes, risk level and a SHA-256 checksum. The checksum is computed here from
 * the skill body the user typed, so the value that reaches the server is the value of that body.
 */
export function SkillsPage() {
  const skills = useApiData(() => api.listSkills(), []);
  const [dialogOpen, setDialogOpen] = useState(false);
  const [submitting, setSubmitting] = useState(false);

  const [skillId, setSkillId] = useState("");
  const [name, setName] = useState("");
  const [version, setVersion] = useState("1.0.0");
  const [description, setDescription] = useState("");
  const [kind, setKind] = useState<SkillKind>("engineering");
  const [riskLevel, setRiskLevel] = useState<RiskLevel>("low");
  const [approvalPolicy, setApprovalPolicy] = useState<"auto" | "require_approval">("require_approval");
  const [permissions, setPermissions] = useState<string[]>(["read_file"]);
  const [allowedRuntimes, setAllowedRuntimes] = useState("*");
  const [content, setContent] = useState("");

  const togglePermission = (permission: string) => {
    setPermissions(current =>
      current.includes(permission)
        ? current.filter(item => item !== permission)
        : [...current, permission]
    );
  };

  const submit = async () => {
    setSubmitting(true);
    try {
      const checksum = await sha256Hex(content);
      await api.registerSkill({
        skillId: skillId.trim(),
        version: version.trim(),
        name: name.trim(),
        ...(description.trim() ? { description: description.trim() } : {}),
        kind,
        permissions,
        allowedRuntimes: allowedRuntimes
          .split(/[\n,]/)
          .map(item => item.trim())
          .filter(item => item !== ""),
        riskLevel,
        approvalPolicy,
        checksum,
        createdAt: new Date().toISOString()
      });
      toast.success(`技能 ${skillId.trim()} 已注册`);
      setDialogOpen(false);
      skills.refresh();
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className="space-y-4">
      <Card className="border-border/70 bg-card/60">
        <CardHeader className="flex-row items-start justify-between space-y-0">
          <div>
            <CardTitle className="text-base">技能清单</CardTitle>
            <CardDescription>
              技能必须声明所需权限、可运行环境、风险等级与审批策略；未声明的一律拒绝。
            </CardDescription>
          </div>
          <Button size="sm" onClick={() => setDialogOpen(true)}>
            <Plus className="size-4" />
            注册技能
          </Button>
        </CardHeader>
        <CardContent className="space-y-3">
          {skills.error ? <ErrorNotice error={skills.error} onRetry={skills.refresh} /> : null}
          {skills.loading ? <p className="text-sm text-muted-foreground">读取中…</p> : null}

          {skills.data && skills.data.skills.length === 0 ? (
            <EmptyState
              icon={Puzzle}
              title="技能库为空"
              description="注册一个技能后，它才能被绑定到执行并按策略授权。"
            />
          ) : null}

          {skills.data && skills.data.skills.length > 0 ? (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>技能</TableHead>
                  <TableHead>类型</TableHead>
                  <TableHead>风险</TableHead>
                  <TableHead>审批</TableHead>
                  <TableHead>权限</TableHead>
                  <TableHead>运行环境</TableHead>
                  <TableHead>注册时间</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {skills.data.skills.map(skill => (
                  <TableRow key={`${skill.skillId}@${skill.version}`}>
                    <TableCell>
                      <div className="flex items-center gap-2">
                        <div className="min-w-0">
                          <p className="truncate text-sm font-medium">{skill.name}</p>
                          <p className="truncate font-mono text-xs text-muted-foreground">
                            {skill.skillId}@{skill.version}
                          </p>
                        </div>
                        {skill.deprecated ? (
                          <Badge variant="outline" className="border-amber-500/40 bg-amber-500/15 text-amber-300">
                            已弃用
                          </Badge>
                        ) : null}
                      </div>
                    </TableCell>
                    <TableCell className="text-xs">{skill.kind}</TableCell>
                    <TableCell>
                      <Badge variant="outline" className={RISK_TONE[skill.riskLevel]}>
                        {skill.riskLevel}
                      </Badge>
                    </TableCell>
                    <TableCell className="text-xs">
                      {skill.approvalPolicy === "auto" ? "自动授权" : "需要审批"}
                    </TableCell>
                    <TableCell className="text-xs text-muted-foreground">
                      {skill.permissions.join(", ") || "-"}
                    </TableCell>
                    <TableCell className="text-xs text-muted-foreground">
                      {skill.allowedRuntimes.join(", ")}
                    </TableCell>
                    <TableCell className="text-xs text-muted-foreground">
                      {formatTime(skill.createdAt)}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          ) : null}
        </CardContent>
      </Card>

      <Dialog open={dialogOpen} onOpenChange={setDialogOpen}>
        <DialogContent className="flex max-h-[85vh] flex-col sm:max-w-2xl">
          <DialogHeader>
            <DialogTitle>注册技能</DialogTitle>
            <DialogDescription>
              校验和由客户端按技能正文计算（SHA-256）。技能正文本身由 Runtime 侧加载，此处只登记清单。
            </DialogDescription>
          </DialogHeader>

          <div className="grid flex-1 gap-4 overflow-y-auto">
            <div className="grid grid-cols-2 gap-3">
              <div className="space-y-2">
                <Label htmlFor="skill-id">技能 ID</Label>
                <Input
                  id="skill-id"
                  value={skillId}
                  placeholder="doc-review"
                  onChange={event => setSkillId(event.target.value)}
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="skill-name">名称</Label>
                <Input
                  id="skill-name"
                  value={name}
                  placeholder="文档评审"
                  onChange={event => setName(event.target.value)}
                />
              </div>
            </div>

            <div className="grid grid-cols-3 gap-3">
              <div className="space-y-2">
                <Label htmlFor="skill-version">版本</Label>
                <Input
                  id="skill-version"
                  value={version}
                  onChange={event => setVersion(event.target.value)}
                />
              </div>
              <div className="space-y-2">
                <Label>类型</Label>
                <Select value={kind} onValueChange={value => setKind(value as SkillKind)}>
                  <SelectTrigger>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="platform">platform</SelectItem>
                    <SelectItem value="engineering">engineering</SelectItem>
                    <SelectItem value="hermes-native">hermes-native</SelectItem>
                  </SelectContent>
                </Select>
              </div>
              <div className="space-y-2">
                <Label>风险等级</Label>
                <Select value={riskLevel} onValueChange={value => setRiskLevel(value as RiskLevel)}>
                  <SelectTrigger>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="low">low</SelectItem>
                    <SelectItem value="medium">medium</SelectItem>
                    <SelectItem value="high">high</SelectItem>
                  </SelectContent>
                </Select>
              </div>
            </div>

            <div className="space-y-2">
              <Label>所需权限</Label>
              <div className="flex flex-wrap gap-2">
                {KNOWN_PERMISSIONS.map(permission => (
                  <button
                    key={permission}
                    type="button"
                    onClick={() => togglePermission(permission)}
                    className={`rounded-md border px-2 py-1 text-xs transition-colors ${
                      permissions.includes(permission)
                        ? "border-brand/60 bg-brand/15 text-brand"
                        : "border-border/70 text-muted-foreground hover:border-border"
                    }`}
                  >
                    {permission}
                  </button>
                ))}
              </div>
            </div>

            <div className="grid grid-cols-2 gap-3">
              <div className="space-y-2">
                <Label htmlFor="runtimes">可运行环境（逗号分隔）</Label>
                <Input
                  id="runtimes"
                  value={allowedRuntimes}
                  onChange={event => setAllowedRuntimes(event.target.value)}
                />
              </div>
              <div className="space-y-2">
                <Label>审批策略</Label>
                <Select
                  value={approvalPolicy}
                  onValueChange={value => setApprovalPolicy(value as "auto" | "require_approval")}
                >
                  <SelectTrigger>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="auto">auto</SelectItem>
                    <SelectItem value="require_approval">require_approval</SelectItem>
                  </SelectContent>
                </Select>
              </div>
            </div>

            <div className="space-y-2">
              <Label htmlFor="skill-description">描述</Label>
              <Input
                id="skill-description"
                value={description}
                onChange={event => setDescription(event.target.value)}
              />
            </div>

            <div className="space-y-2">
              <Label htmlFor="skill-content">技能正文（用于计算校验和）</Label>
              <Textarea
                id="skill-content"
                rows={5}
                value={content}
                placeholder="# 技能说明"
                onChange={event => setContent(event.target.value)}
              />
              <p className="flex items-center gap-1.5 text-xs text-muted-foreground">
                <ShieldAlert className="size-3.5" />
                正文不会被上传；只有它的 SHA-256 会随清单提交。
              </p>
            </div>
          </div>

          <DialogFooter className="shrink-0">
            <Button variant="ghost" onClick={() => setDialogOpen(false)}>
              取消
            </Button>
            <Button onClick={() => void submit()} disabled={submitting}>
              {submitting ? <Loader2 className="size-4 animate-spin" /> : <TriangleAlert className="size-4" />}
              注册
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
