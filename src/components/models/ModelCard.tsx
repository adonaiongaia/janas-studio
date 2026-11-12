import { Download, Heart, TriangleAlert, X } from "lucide-react";

import { cn } from "../../lib/cn";
import {
  architectureFor,
  formatBytes,
  formatCount,
  formatParameters,
  formatRelativeDate,
  gradientFor,
  initials,
} from "../../lib/format";
import type { DownloadProgress, HfModel } from "../../types";
import { Button } from "../ui/Button";
import { ProgressBar } from "../ui/ProgressBar";

interface ModelCardProps {
  model: HfModel;
  download?: DownloadProgress;
  onOpen: (model: HfModel) => void;
  onCancel: (id: string) => void;
}

export function ModelCard({
  model,
  download,
  onOpen,
  onCancel,
}: ModelCardProps) {
  const primary = model.quants[0];
  const running =
    download?.status === "preparing" || download?.status === "downloading";
  const failed = download?.status === "error";
  const complete = download?.status === "complete";

  return (
    <article className="flex animate-fade-in flex-col rounded-2xl border border-white/10 bg-ink-900/60 p-4 transition hover:border-white/20 hover:bg-ink-900">
      <div className="flex items-start gap-3">
        <div
          className={cn(
            "flex h-12 w-12 shrink-0 items-center justify-center rounded-xl border border-white/10 bg-gradient-to-br text-sm font-semibold text-white",
            gradientFor(model.id),
          )}
        >
          {initials(model.name)}
        </div>
        <div className="min-w-0 flex-1">
          <p className="truncate text-[10px] uppercase tracking-[0.12em] text-slate-500">
            {model.author}
          </p>
          <h3 className="mt-0.5 truncate text-[13px] font-semibold text-slate-100">
            {model.name}
          </h3>
          <div className="mt-2 flex flex-wrap gap-1.5">
            <span className="rounded-md border border-acid-400/20 bg-acid-400/10 px-1.5 py-0.5 font-mono text-[9px] font-medium text-acid-300">
              {primary?.quant ?? "GGUF"}
            </span>
            <span
              title={model.pipeline_tag ?? undefined}
              className="truncate rounded-md border border-white/10 bg-white/5 px-1.5 py-0.5 text-[9px] text-slate-400"
            >
              {architectureFor(model)}
            </span>
            {model.parameters ? (
              <span className="rounded-md border border-white/10 bg-white/5 px-1.5 py-0.5 text-[9px] text-slate-400">
                {formatParameters(model.parameters)}
              </span>
            ) : null}
          </div>
        </div>
      </div>

      <div className="mt-4 flex items-center gap-4 text-[10px] text-slate-500">
        <span className="inline-flex items-center gap-1">
          <Download className="h-3 w-3" />
          {formatCount(model.downloads)}
        </span>
        <span className="inline-flex items-center gap-1">
          <Heart className="h-3 w-3" />
          {formatCount(model.likes)}
        </span>
        <span className="ml-auto">{formatRelativeDate(model.last_modified)}</span>
      </div>

      <div className="mt-3 flex items-center justify-between gap-3 rounded-xl border border-white/5 bg-ink-950/50 px-3 py-2">
        <span className="truncate text-[10px] text-slate-500">
          {primary
            ? `${primary.files.length} file${primary.files.length === 1 ? "" : "s"} · ${formatBytes(
                primary.size_bytes,
              )}`
            : "No convertible GGUF files"}
        </span>
        <span className="shrink-0 font-mono text-[10px] text-slate-400">
          {primary?.quant ?? "—"}
        </span>
      </div>

      {running && download ? (
        <div className="mt-3 space-y-2">
          <div className="flex items-center justify-between text-[10px]">
            <span className="truncate text-slate-400">{download.message}</span>
            <span className="ml-2 shrink-0 font-mono text-acid-300">
              {download.percent !== null
                ? `${download.percent.toFixed(0)}%`
                : "…"}
            </span>
          </div>
          <ProgressBar value={download.percent} />
          <button
            type="button"
            onClick={() => onCancel(download.id)}
            className="inline-flex items-center gap-1 text-[10px] text-slate-500 transition hover:text-rose-300"
          >
            <X className="h-3 w-3" />
            Cancel download
          </button>
        </div>
      ) : failed && download ? (
        <div className="mt-3 space-y-2">
          <p className="flex items-start gap-1.5 rounded-lg border border-rose-400/20 bg-rose-500/5 px-2.5 py-2 text-[10px] leading-4 text-rose-300">
            <TriangleAlert className="mt-0.5 h-3 w-3 shrink-0" />
            {download.message}
          </p>
          <Button
            size="sm"
            variant="secondary"
            className="w-full justify-center"
            onClick={() => onOpen(model)}
          >
            Try again
          </Button>
        </div>
      ) : (
        <div className="mt-3 space-y-1.5">
          <Button
            size="sm"
            variant={complete ? "secondary" : "primary"}
            className="w-full justify-center"
            disabled={!primary}
            onClick={() => onOpen(model)}
            icon={<Download className="h-3.5 w-3.5" />}
          >
            {complete ? "Downloaded · download again" : "Download"}
          </Button>
          {complete ? (
            <p className="truncate text-center text-[9px] text-emerald-400/80">
              Saved to {download.saved_path ?? "your models folder"}
            </p>
          ) : null}
        </div>
      )}
    </article>
  );
}
