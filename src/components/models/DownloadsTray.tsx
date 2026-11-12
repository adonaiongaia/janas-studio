import { Pause, Play, X } from "lucide-react";

import { cn } from "../../lib/cn";
import { formatBytes } from "../../lib/format";
import type { DownloadProgress } from "../../types";
import { ProgressBar } from "../ui/ProgressBar";

interface DownloadsTrayProps {
  downloads: Record<string, DownloadProgress>;
  onCancel: (id: string) => void;
  onPause: (id: string) => void;
  onResume: (id: string) => void;
  onClearFinished: () => void;
}

const STATUS_STYLE: Record<string, string> = {
  preparing: "text-slate-400",
  downloading: "text-acid-300",
  paused: "text-amber-300",
  complete: "text-emerald-300",
  cancelled: "text-amber-300",
  error: "text-rose-300",
};

export function DownloadsTray({
  downloads,
  onCancel,
  onPause,
  onResume,
  onClearFinished,
}: DownloadsTrayProps) {
  const items = Object.values(downloads);
  if (!items.length) return null;

  const active = items.filter((item) =>
    ["preparing", "downloading", "paused"].includes(item.status),
  ).length;
  const finished = items.length - active;

  return (
    <div className="fixed bottom-5 right-5 z-40 w-[380px] overflow-hidden rounded-2xl border border-white/10 bg-ink-900 shadow-panel animate-fade-in">
      <div className="flex items-center justify-between border-b border-white/5 px-4 py-3">
        <div>
          <p className="text-[11px] font-semibold text-slate-200">Downloads</p>
          <p className="text-[9px] text-slate-600">
            {active} active · {finished} finished
          </p>
        </div>
        {finished > 0 ? (
          <button
            type="button"
            onClick={onClearFinished}
            className="text-[10px] text-slate-500 transition hover:text-slate-300"
          >
            Clear finished
          </button>
        ) : null}
      </div>

      <div className="max-h-[340px] divide-y divide-white/5 overflow-y-auto">
        {items.map((download) => {
          const running = ["preparing", "downloading"].includes(
            download.status,
          );
          const paused = download.status === "paused";
          return (
            <div key={download.id} className="px-4 py-3">
              <div className="flex items-center justify-between gap-2">
                <p className="truncate text-[11px] font-medium text-slate-300">
                  {download.repo}
                </p>
                <span
                  className={cn(
                    "shrink-0 text-[9px] uppercase tracking-wider",
                    STATUS_STYLE[download.status] ?? "text-slate-500",
                  )}
                >
                  {download.status}
                </span>
              </div>
              <p className="mt-0.5 truncate text-[10px] text-slate-600">
                {download.message}
              </p>
              <div className="mt-2 flex items-center gap-2">
                <ProgressBar
                  value={download.percent}
                  tone={
                    download.status === "error"
                      ? "rose"
                      : paused
                        ? "sky"
                        : "acid"
                  }
                  className="flex-1"
                />
                <span className="w-14 shrink-0 text-right font-mono text-[9px] text-slate-500">
                  {download.downloaded > 0
                    ? formatBytes(download.downloaded)
                    : download.percent !== null
                      ? `${download.percent.toFixed(0)}%`
                      : "—"}
                </span>
                {running || paused ? (
                  <div className="flex shrink-0 items-center gap-0.5">
                    <button
                      type="button"
                      onClick={() =>
                        paused ? onResume(download.id) : onPause(download.id)
                      }
                      className={cn(
                        "rounded p-1 transition",
                        paused
                          ? "text-acid-300 hover:bg-acid-400/10"
                          : "text-slate-500 hover:bg-white/5 hover:text-slate-200",
                      )}
                      aria-label={paused ? "Resume download" : "Pause download"}
                      title={paused ? "Resume" : "Pause"}
                    >
                      {paused ? (
                        <Play className="h-3 w-3" />
                      ) : (
                        <Pause className="h-3 w-3" />
                      )}
                    </button>
                    <button
                      type="button"
                      onClick={() => onCancel(download.id)}
                      className="rounded p-1 text-slate-600 transition hover:text-rose-300"
                      aria-label="Cancel download"
                      title="Cancel"
                    >
                      <X className="h-3 w-3" />
                    </button>
                  </div>
                ) : null}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
