import { Badge } from "@/components/ui/badge";
import { statusLabel, statusTone } from "@/lib/format";

const TONE_CLASS: Record<string, string> = {
  success: "border-emerald-500/40 bg-emerald-500/15 text-emerald-300 dark:text-emerald-300",
  warning: "border-amber-500/40 bg-amber-500/15 text-amber-300 dark:text-amber-300",
  danger: "border-red-500/40 bg-red-500/15 text-red-300 dark:text-red-300",
  info: "border-sky-500/40 bg-sky-500/15 text-sky-300 dark:text-sky-300",
  neutral: "border-border bg-muted text-muted-foreground"
};

/** Status pill. The label is translated, the raw status stays available as the title. */
export function StatusBadge({ status, className }: { status?: string; className?: string }) {
  const tone = statusTone(status);
  return (
    <Badge
      variant="outline"
      title={status ?? undefined}
      className={`${TONE_CLASS[tone]} ${className ?? ""}`}
    >
      {statusLabel(status)}
    </Badge>
  );
}
