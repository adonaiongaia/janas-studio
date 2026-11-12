import { cn } from "../../lib/cn";
import type { EngineStatus } from "../../types";

const META: Record<
  EngineStatus,
  { label: string; dot: string; text: string; border: string; bg: string }
> = {
  disconnected: {
    label: "Disconnected",
    dot: "bg-slate-500",
    text: "text-slate-400",
    border: "border-white/10",
    bg: "bg-white/5",
  },
  indexing: {
    label: "Indexing",
    dot: "bg-amber-400",
    text: "text-amber-300",
    border: "border-amber-400/20",
    bg: "bg-amber-400/10",
  },
  loading_model: {
    label: "Loading Model",
    dot: "bg-sky-400",
    text: "text-sky-300",
    border: "border-sky-400/20",
    bg: "bg-sky-400/10",
  },
  ready: {
    label: "Ready",
    dot: "bg-emerald-400",
    text: "text-emerald-300",
    border: "border-emerald-400/20",
    bg: "bg-emerald-400/10",
  },
};

interface StatusPillProps {
  status: EngineStatus;
  detail?: string;
  className?: string;
}

export function StatusPill({ status, detail, className }: StatusPillProps) {
  const meta = META[status];
  const active = status === "indexing" || status === "loading_model";
  return (
    <span
      title={detail}
      className={cn(
        "inline-flex items-center gap-2 rounded-full border px-2.5 py-1 text-[11px] font-medium",
        meta.border,
        meta.bg,
        meta.text,
        className,
      )}
    >
      <span className="relative flex h-1.5 w-1.5">
        {active ? (
          <span
            className={cn(
              "absolute inline-flex h-full w-full animate-ping rounded-full opacity-60",
              meta.dot,
            )}
          />
        ) : null}
        <span className={cn("relative h-1.5 w-1.5 rounded-full", meta.dot)} />
      </span>
      {meta.label}
    </span>
  );
}
