import type { LucideIcon } from "lucide-react";
import { Card, CardContent } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";

export interface StatCardProps {
  label: string;
  value: string;
  hint?: string;
  icon?: LucideIcon;
  loading?: boolean;
  tone?: "default" | "brand";
}

/** One number with its label. Used across the overview so the layout stays consistent. */
export function StatCard({ label, value, hint, icon: Icon, loading, tone = "default" }: StatCardProps) {
  return (
    <Card className="border-border/70 bg-card/60">
      <CardContent className="flex items-start justify-between gap-3 p-4">
        <div className="min-w-0">
          <p className="text-xs font-medium tracking-wide text-muted-foreground uppercase">{label}</p>
          {loading ? (
            <Skeleton className="mt-2 h-7 w-20" />
          ) : (
            <p
              className={`mt-1 truncate text-2xl font-semibold tabular-nums ${
                tone === "brand" ? "text-brand" : "text-foreground"
              }`}
            >
              {value}
            </p>
          )}
          {hint ? <p className="mt-1 truncate text-xs text-muted-foreground">{hint}</p> : null}
        </div>
        {Icon ? (
          <span className="rounded-lg border border-border/70 bg-background/60 p-2 text-muted-foreground">
            <Icon className="size-4" />
          </span>
        ) : null}
      </CardContent>
    </Card>
  );
}
