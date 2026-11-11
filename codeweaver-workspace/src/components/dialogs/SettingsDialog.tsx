import { useState } from "react";
import { CheckCircle2, Loader2, Plug } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
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
import { api } from "@/lib/api";
import { DEFAULT_API_BASE_URL, useConnection } from "@/lib/settings";

/**
 * Backend connection settings.
 *
 * The lab server picks its port and its bearer token from the environment, so the desktop client
 * cannot know them at build time. The dialog therefore stores them locally and can verify them
 * against the real server before the user leaves the dialog.
 */
export function SettingsDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const { baseUrl, token, setBaseUrl, setToken } = useConnection();
  const [draftUrl, setDraftUrl] = useState(baseUrl);
  const [draftToken, setDraftToken] = useState(token);
  const [checking, setChecking] = useState(false);

  const save = () => {
    setBaseUrl(draftUrl);
    setToken(draftToken);
    toast.success("已保存连接设置");
    onOpenChange(false);
  };

  const test = async () => {
    setChecking(true);
    try {
      setBaseUrl(draftUrl);
      setToken(draftToken);
      const { runtimes } = await api.listRuntimes();
      toast.success(`连接成功，已注册 ${runtimes.length} 个 Runtime`);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      setChecking(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>连接设置</DialogTitle>
          <DialogDescription>
            指向 CodeWeaver lab server（默认 {DEFAULT_API_BASE_URL}）。地址与令牌由服务端的
            CODEWEAVER_API_PORT / CODEWEAVER_API_TOKEN 决定。
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4">
          <div className="space-y-2">
            <Label htmlFor="base-url">服务地址</Label>
            <Input
              id="base-url"
              value={draftUrl}
              placeholder={DEFAULT_API_BASE_URL}
              onChange={event => setDraftUrl(event.target.value)}
            />
          </div>

          <div className="space-y-2">
            <Label htmlFor="api-token">Bearer Token（可选）</Label>
            <Input
              id="api-token"
              type="password"
              value={draftToken}
              placeholder="服务端未开启鉴权时留空"
              onChange={event => setDraftToken(event.target.value)}
            />
          </div>

          <Button variant="outline" onClick={() => void test()} disabled={checking}>
            {checking ? <Loader2 className="size-4 animate-spin" /> : <Plug className="size-4" />}
            测试连接
          </Button>
        </div>

        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            取消
          </Button>
          <Button onClick={save}>
            <CheckCircle2 className="size-4" />
            保存
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
