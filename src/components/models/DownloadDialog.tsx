import { useEffect, useMemo, useState } from "react";
import {
  AlertTriangle,
  Download,
  FolderTree,
  Pause,
  Play,
  X,
} from "lucide-react";

import { api } from "../../lib/api";
import { cn } from "../../lib/cn";
import { errorMessage, formatBytes } from "../../lib/format";
import type { DownloadProgress, HfModel, HfQuant } from "../../types";
import { Button } from "../ui/Button";
import { ProgressBar } from "../ui/ProgressBar";

interface DownloadDialogProps {
  model: HfModel;
  modelsDir: string;
  downloads: Record<string, DownloadProgress>;
  onClose: () => void;
  onDownload: (quant: HfQuant) => Promise<string>;
  onPause: (id: string) => void;
  onResume: (id: string) => void;
  onCancel: (id: string) => void;
}

export function DownloadDialog({
  model,
  modelsDir,
  downloads,
  onClose,
  onDownload,
  onPause,
  onResume,
  onCancel,
}: DownloadDialogProps) {
  const [quants, setQuants] = useState<HfQuant[]>(model.quants);
  const [selected, setSelected] = useState<string>(
    model.quants[0]?.quant ?? "",
  );
  const [loading, setLoading] = useState(model.quants.length === 0);
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [downloadId, setDownloadId] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    api
      .listModelQuants(model.id)
      .then((fresh) => {
        if (cancelled) return;
        setQuants(fresh);
        if (!fresh.some((quant) => quant.quant === selected)) {
          setSelected(fresh[0]?.quant ?? "");
        }
      })
      .catch((err) => {
        if (!cancelled) setError(errorMessage(err));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
    // Only the repository identity should trigger a refetch.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [model.id]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !starting) onClose();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onClose, starting]);

  const active = useMemo(
    () => quants.find((quant) => quant.quant === selected) ?? quants[0],
    [quants, selected],
  );
  const job = downloadId ? downloads[downloadId] : undefined;
  const running = job ? ["preparing", "downloading"].includes(job.status) : false;
  const paused = job?.status === "paused";
  const finished = job?.status === "complete";

  const destination = `${modelsDir.replace(/\/+$/, "")}/${model.id}`;

  const start = async () => {
    if (!active) return;
    setStarting(true);
    setError(null);
    try {
      const id = await onDownload(active);
      setDownloadId(id);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setStarting(false);
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-ink-950/80 p-6 backdrop-blur-sm"
      role="dialog"
      aria-modal="true"
      aria-label={`Download ${model.name}`}
      onMouseDown={(event) => {
        if (event.target === event.currentTarget && !starting) onClose();
      }}
    >
      <div className="panel w-full max-w-xl overflow-hidden bg-ink-900 shadow-panel animate-fade-in">
        <div className="flex items-start justify-between gap-4 border-b border-white/5 px-5 py-4">
          <div className="min-w-0">
            <p className="text-[10px] uppercase tracking-[0.14em] text-slate-500">
              {model.author}
            </p>
            <h2 className="mt-1 truncate text-base font-semibold text-white">
              {model.name}
            </h2>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="rounded-lg p-1 text-slate-500 transition hover:bg-white/5 hover:text-slate-200"
            aria-label="Close"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        <div className="space-y-4 px-5 py-4">
          {job ? (
            <div className="rounded-xl border border-white/10 bg-ink-950/60 p-3.5">
              <div className="flex items-center justify-between gap-2">
                <p className="text-[11px] font-medium text-slate-200">
                  {job.repo}
                </p>
                <span
                  className={cn(
                    "text-[9px] uppercase tracking-wider",
                    paused
                      ? "text-amber-300"
                      : finished
                        ? "text-emerald-300"
                        : job.status === "error"
                          ? "text-rose-300"
                          : "text-acid-300",
                  )}
                >
                  {job.status}
                </span>
              </div>
              <p className="mt-0.5 truncate text-[10px] text-slate-600">
                {job.message}
              </p>
              <div className="mt-2 flex items-center gap-2">
                <ProgressBar
                  value={job.percent}
                  tone={
                    job.status === "error"
                      ? "rose"
                      : paused
                        ? "sky"
                        : "acid"
                  }
                  className="flex-1"
                />
                <span className="w-16 text-right font-mono text-[9px] text-slate-500">
                  {formatBytes(job.downloaded)} / {formatBytes(job.total)}
                </span>
              </div>
              <div className="mt-2.5 flex items-center gap-2">
                {running || paused ? (
                  <>
                    <Button
                      size="sm"
                      variant={paused ? "primary" : "secondary"}
                      onClick={() =>
                        paused ? onResume(job.id) : onPause(job.id)
                      }
                      icon={
                        paused ? (
                          <Play className="h-3 w-3" />
                        ) : (
                          <Pause className="h-3 w-3" />
                        )
                      }
                    >
                      {paused ? "Resume" : "Pause"}
                    </Button>
                    <Button
                      size="sm"
                      variant="ghost"
                      onClick={() => onCancel(job.id)}
                    >
                      Cancel
                    </Button>
                  </>
                ) : null}
                {finished ? (
                  <p className="text-[10px] leading-4 text-emerald-300/80">
                    Files are in your models folder. Compile them to .jns next.
                  </p>
                ) : null}
                {job.status === "error" ? (
                  <p className="text-[10px] leading-4 text-rose-300/80">
                    {job.message}
                  </p>
                ) : null}
              </div>
            </div>
          ) : null}

          {!job ? (
            <>
              <div className="flex items-center justify-between">
                <span className="label">Quantization</span>
                {loading ? (
                  <span className="text-[10px] text-slate-500">
                    refreshing file list…
                  </span>
                ) : (
                  <span className="text-[10px] text-slate-500">
                    {quants.length} available
                  </span>
                )}
              </div>

              {error ? (
                <div className="flex items-start gap-2 rounded-xl border border-amber-400/20 bg-amber-400/5 px-3 py-2 text-[10px] leading-4 text-amber-200">
                  <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
                  <span>{error}</span>
                </div>
              ) : null}

              <div className="max-h-52 space-y-1.5 overflow-y-auto pr-1">
                {quants.length ? (
                  quants.map((quant) => (
                    <button
                      key={quant.quant}
                      type="button"
                      onClick={() => setSelected(quant.quant)}
                      className={cn(
                        "flex w-full items-center justify-between gap-3 rounded-xl border px-3 py-2.5 text-left transition",
                        active?.quant === quant.quant
                          ? "border-acid-400/40 bg-acid-400/5"
                          : "border-white/10 bg-white/[0.02] hover:bg-white/5",
                      )}
                    >
                      <div className="min-w-0">
                        <p className="text-xs font-medium text-slate-200">
                          {quant.quant}
                        </p>
                        <p className="text-[10px] text-slate-500">
                          {quant.files.length} file
                          {quant.files.length === 1 ? "" : "s"}
                        </p>
                      </div>
                      <span className="shrink-0 font-mono text-[10px] text-slate-400">
                        {formatBytes(quant.size_bytes)}
                      </span>
                    </button>
                  ))
                ) : (
                  <p className="rounded-xl border border-white/5 bg-white/[0.02] px-3 py-6 text-center text-[11px] text-slate-500">
                    {loading
                      ? "Looking for GGUF files…"
                      : "This repository has no convertible GGUF files."}
                  </p>
                )}
              </div>

              {active ? (
                <>
                  <div className="rounded-xl border border-white/5 bg-ink-950/50 px-3 py-2.5">
                    <div className="flex items-center gap-2 text-[10px] text-slate-500">
                      <FolderTree className="h-3 w-3" />
                      <span className="truncate">{destination}</span>
                    </div>
                    <div className="mt-2 max-h-20 overflow-y-auto font-mono text-[9px] leading-4 text-slate-600">
                      {active.files.map((file) => (
                        <p key={file.path} className="truncate">
                          {file.path}{" "}
                          <span className="text-slate-700">
                            ({formatBytes(file.size)})
                          </span>
                        </p>
                      ))}
                    </div>
                  </div>

                  <div className="rounded-xl border border-white/5 bg-white/[0.02] px-3 py-2.5">
                    <p className="label">Memory estimate</p>
                    <div className="mt-2 grid grid-cols-2 gap-2">
                      <MemoryStat
                        label="System RAM"
                        value={formatBytes(
                          active.size_bytes * 1.15 + 1.5 * 1024 ** 3,
                        )}
                        hint="working set at load"
                      />
                      <MemoryStat
                        label="VRAM"
                        value={formatBytes(active.size_bytes * 1.05)}
                        hint="if fully offloaded"
                      />
                    </div>
                    <p className="mt-1.5 text-[9px] leading-4 text-slate-600">
                      Rough estimate: weights plus a 1.5 GiB margin for KV cache
                      and runtime. A mixture of experts streams unused experts
                      from disk, so resident RAM can be far lower.
                    </p>
                  </div>
                </>
              ) : null}

              {starting ? <ProgressBar value={null} /> : null}
            </>
          ) : null}
        </div>

        <div className="flex items-center justify-between gap-3 border-t border-white/5 bg-ink-850/60 px-5 py-3.5">
          <p className="text-[10px] text-slate-600">
            Pause keeps the stream open; resume continues at the same byte.
          </p>
          <div className="flex items-center gap-2">
            <Button size="sm" variant="ghost" onClick={onClose}>
              {job && !finished ? "Run in background" : "Close"}
            </Button>
            {!job || job.status === "error" ? (
              <Button
                size="sm"
                variant="primary"
                busy={starting}
                disabled={!active}
                onClick={() => void start()}
                icon={<Download className="h-3.5 w-3.5" />}
              >
                {active
                  ? `Download ${formatBytes(active.size_bytes)}`
                  : "Download"}
              </Button>
            ) : null}
          </div>
        </div>
      </div>
    </div>
  );
}

function MemoryStat({
  label,
  value,
  hint,
}: {
  label: string;
  value: string;
  hint: string;
}) {
  return (
    <div className="rounded-lg border border-white/5 bg-ink-950/60 px-2.5 py-1.5">
      <p className="text-[9px] uppercase tracking-wider text-slate-600">
        {label}
      </p>
      <p className="mt-0.5 font-mono text-[11px] text-slate-300">{value}</p>
      <p className="text-[8px] text-slate-700">{hint}</p>
    </div>
  );
}
