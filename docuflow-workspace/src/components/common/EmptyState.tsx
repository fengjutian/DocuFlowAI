import type { LucideIcon } from "lucide-react";
import { AlertCircle, Inbox } from "lucide-react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { cn } from "cn";

export function EmptyState({
  icon: Icon = Inbox,
  title,
  description,
  action
}: {
  icon?: LucideIcon;
  title: string;
  description?: string;
  action?: React.ReactNode;
}) {
  return (
    <div className="flex flex-col items-center justify-center gap-2 rounded-lg border border-dashed border-border/70 bg-background/40 px-6 py-12 text-center">
      <Icon className="size-6 text-muted-foreground" />
      <p className="text-sm font-medium">{title}</p>
      {description ? <p className="max-w-md text-xs text-muted-foreground">{description}</p> : null}
      {action}
    </div>
  );
}

/** Renders a failed request. The message is the server's own words, never a generic apology. */
export function ErrorNotice({
  error,
  onRetry,
  className
}: {
  error: unknown;
  onRetry?: () => void;
  className?: string;
}) {
  const message = error instanceof Error ? error.message : String(error);
  return (
    <Alert variant="destructive" className={cn("items-start", className)}>
      <AlertCircle className="size-4" />
      <div className="min-w-0 flex-1">
        <AlertTitle>请求失败</AlertTitle>
        <AlertDescription className="break-words">{message}</AlertDescription>
      </div>
      {onRetry ? (
        <button
          type="button"
          onClick={onRetry}
          className="shrink-0 rounded-md border border-destructive/40 px-2 py-1 text-xs hover:bg-destructive/10"
        >
          重试
        </button>
      ) : null}
    </Alert>
  );
}
