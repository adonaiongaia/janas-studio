import { cn } from "../../lib/cn";

interface ProgressBarProps {
  value: number | null;
  className?: string;
  tone?: "acid" | "sky" | "rose";
  indeterminate?: boolean;
}

const TONES = {
  acid: "bg-acid-400",
  sky: "bg-sky-400",
  rose: "bg-rose-400",
};

export function ProgressBar({
  value,
  className,
  tone = "acid",
  indeterminate = false,
}: ProgressBarProps) {
  const clamped =
    value === null ? null : Math.min(100, Math.max(0, Number(value)));
  return (
    <div
      className={cn(
        "relative h-1.5 w-full overflow-hidden rounded-full bg-white/10",
        className,
      )}
      role="progressbar"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={clamped ?? undefined}
    >
      {clamped === null || indeterminate ? (
        <div
          className={cn(
            "progress-indeterminate h-full w-1/3 rounded-full opacity-80",
            TONES[tone],
          )}
        />
      ) : (
        <div
          className={cn("h-full rounded-full transition-[width] duration-200", TONES[tone])}
          style={{ width: `${clamped}%` }}
        />
      )}
    </div>
  );
}
