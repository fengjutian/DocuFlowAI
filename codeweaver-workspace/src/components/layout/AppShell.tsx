import { NavLink, useLocation } from "react-router-dom";
import {
  Activity,
  Gauge,
  GitBranch,
  GitCommitHorizontal,
  LayoutDashboard,
  Puzzle,
  Server,
  Settings,
  Waypoints
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "cn";
import { useBackendStatus } from "@/hooks/useBackendStatus";
import { useExecutionStream } from "@/state/execution-stream";
import { ThemeSwitcher } from "@/components/common/ThemeSwitcher";

const NAV_ITEMS = [
  { to: "/", label: "工作台总览", icon: LayoutDashboard, end: true },
  { to: "/runtimes", label: "Runtime 管理", icon: Server, end: false },
  { to: "/executions", label: "执行与事件", icon: Activity, end: false },
  { to: "/subagents", label: "子代理委派", icon: GitBranch, end: false },
  { to: "/skills", label: "技能管理", icon: Puzzle, end: false },
  { to: "/usage", label: "用量统计", icon: Gauge, end: false },
  { to: "/trace", label: "提交追溯", icon: GitCommitHorizontal, end: false }
];

const STREAM_LABEL: Record<string, string> = {
  connecting: "连接中",
  open: "实时",
  polling: "轮询",
  closed: "已断开",
  error: "异常"
};

export function AppShell({
  children,
  onOpenSettings
}: {
  children: React.ReactNode;
  onOpenSettings: () => void;
}) {
  const backend = useBackendStatus();
  const { streamStatus } = useExecutionStream();
  const location = useLocation();
  const current = NAV_ITEMS.find(item =>
    item.end ? location.pathname === item.to : location.pathname.startsWith(item.to)
  );

  return (
    <div className="flex h-full w-full overflow-hidden bg-background">
      <aside className="flex w-60 shrink-0 flex-col border-r border-sidebar-border bg-sidebar/60">
        <div className="flex items-center gap-2 px-4 py-4">
          <span className="flex size-8 items-center justify-center rounded-lg bg-brand/15 text-brand">
            <Waypoints className="size-4" />
          </span>
          <div className="min-w-0">
            <p className="truncate text-sm font-semibold">CodeWeaver</p>
            <p className="truncate text-xs text-muted-foreground">代码织造</p>
          </div>
        </div>

        <Separator className="bg-sidebar-border" />

        <nav className="flex-1 space-y-1 overflow-y-auto p-3">
          {NAV_ITEMS.map(item => (
            <NavLink
              key={item.to}
              to={item.to}
              end={item.end}
              className={({ isActive }) =>
                cn(
                  "flex items-center gap-2.5 rounded-lg px-2.5 py-2 text-sm transition-colors",
                  isActive
                    ? "bg-brand/12 text-brand"
                    : "text-sidebar-foreground/80 hover:bg-sidebar-accent hover:text-sidebar-accent-foreground"
                )
              }
            >
              <item.icon className="size-4 shrink-0" />
              <span className="truncate">{item.label}</span>
            </NavLink>
          ))}
        </nav>

        <div className="space-y-2 border-t border-sidebar-border p-3">
          <Tooltip>
            <TooltipTrigger asChild>
              <div className="flex items-center gap-2 rounded-lg px-2 py-1.5 text-xs text-muted-foreground">
                <span
                  className={cn(
                    "size-2 shrink-0 rounded-full",
                    backend.status === "online"
                      ? "bg-emerald-500"
                      : backend.status === "offline"
                        ? "bg-red-500"
                        : "bg-amber-500"
                  )}
                />
                <span className="truncate">
                  {backend.status === "online"
                    ? "后端已连接"
                    : backend.status === "offline"
                      ? "后端未连接"
                      : "检测中"}
                </span>
                <span className="ml-auto truncate font-mono text-[10px] opacity-70">
                  {STREAM_LABEL[streamStatus] ?? streamStatus}
                </span>
              </div>
            </TooltipTrigger>
            <TooltipContent side="top" className="max-w-xs">
              <p className="font-mono text-xs">{backend.baseUrl}</p>
              {backend.message ? <p className="mt-1 text-xs opacity-80">{backend.message}</p> : null}
            </TooltipContent>
          </Tooltip>

          <Button variant="ghost" size="sm" className="w-full justify-start" onClick={onOpenSettings}>
            <Settings className="size-4" />
            连接设置
          </Button>
        </div>
      </aside>

      <div className="flex min-w-0 flex-1 flex-col">
        <header className="flex h-14 shrink-0 items-center justify-between gap-4 border-b border-border/70 px-6">
          <div className="min-w-0">
            <h1 className="truncate text-sm font-semibold">{current?.label ?? "CodeWeaver"}</h1>
            <p className="truncate text-xs text-muted-foreground">
              Weaving Ideas Into Software.
            </p>
          </div>
          <div className="flex items-center gap-2">
            <ThemeSwitcher />
            <Button variant="ghost" size="sm" onClick={onOpenSettings}>
              <Settings className="size-4" />
              设置
            </Button>
          </div>
        </header>

        <main className="df-grid min-h-0 flex-1 overflow-y-auto">
          <div className="mx-auto w-full max-w-[1400px] p-6">{children}</div>
        </main>
      </div>
    </div>
  );
}
