import { useCallback, useEffect, useState } from "react";
import { AlertTriangle, RefreshCw, Search, Sparkles } from "lucide-react";

import { useSettings } from "../../hooks/useSettings";
import { api } from "../../lib/api";
import { cn } from "../../lib/cn";
import { errorMessage } from "../../lib/format";
import type {
  DownloadProgress,
  DownloadRequest,
  HfModel,
  HfQuant,
} from "../../types";
import { Button } from "../ui/Button";
import { DownloadDialog } from "./DownloadDialog";
import { ModelCard } from "./ModelCard";

interface ModelsViewProps {
  downloads: Record<string, DownloadProgress>;
  onStart: (request: DownloadRequest) => Promise<string>;
  onCancel: (id: string) => Promise<void>;
  onPause: (id: string) => Promise<void>;
  onResume: (id: string) => Promise<void>;
}

export function ModelsView({
  downloads,
  onStart,
  onCancel,
  onPause,
  onResume,
}: ModelsViewProps) {
  const { settings } = useSettings();
  const [query, setQuery] = useState("");
  const [ggufOnly, setGgufOnly] = useState(true);
  const [results, setResults] = useState<HfModel[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);
  const [selected, setSelected] = useState<HfModel | null>(null);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);

    const timer = window.setTimeout(() => {
      api
        .searchModels({ query, gguf_only: ggufOnly, limit: 24 })
        .then((models) => {
          if (!cancelled) setResults(models);
        })
        .catch((err) => {
          if (!cancelled) {
            setError(errorMessage(err));
            setResults([]);
          }
        })
        .finally(() => {
          if (!cancelled) setLoading(false);
        });
    }, 350);

    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [query, ggufOnly, refreshKey]);

  const activeDownload = useCallback(
    (repo: string) => {
      const matches = Object.values(downloads).filter(
        (download) => download.repo === repo,
      );
      return (
        matches.find((download) =>
          ["preparing", "downloading"].includes(download.status),
        ) ??
        matches.find((download) => download.status === "error") ??
        matches[matches.length - 1]
      );
    },
    [downloads],
  );

  const downloadQuant = useCallback(
    async (model: HfModel, quant: HfQuant) => {
      return onStart({
        repo: model.id,
        files: quant.files.map((file) => ({
          path: file.path,
          size: file.size,
        })),
      });
    },
    [onStart],
  );

  return (
    <div className="relative flex h-full min-h-0 flex-col">
      <div className="shrink-0 border-b border-white/5 px-6 py-5">
        <div className="flex flex-wrap items-center gap-2.5">
          <div className="relative min-w-[260px] flex-1">
            <Search className="pointer-events-none absolute left-3.5 top-1/2 h-4 w-4 -translate-y-1/2 text-slate-600" />
            <input
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder="Search Hugging Face for GGUF models…"
              className="field pl-10"
              autoComplete="off"
              spellCheck={false}
            />
          </div>

          <button
            type="button"
            onClick={() => setGgufOnly((value) => !value)}
            className={cn(
              "inline-flex h-10 items-center gap-2 rounded-xl border px-3 text-[11px] font-medium transition",
              ggufOnly
                ? "border-acid-400/30 bg-acid-400/10 text-acid-200"
                : "border-white/10 bg-white/5 text-slate-400 hover:text-slate-200",
            )}
            aria-pressed={ggufOnly}
          >
            <span
              className={cn(
                "h-1.5 w-1.5 rounded-full",
                ggufOnly ? "bg-acid-400" : "bg-slate-600",
              )}
            />
            GGUF only
          </button>

          <Button
            variant="secondary"
            onClick={() => setRefreshKey((key) => key + 1)}
            icon={<RefreshCw className={cn("h-3.5 w-3.5", loading && "animate-spin")} />}
          >
            Refresh
          </Button>
        </div>

        <div className="mt-3 flex items-center justify-between text-[10px] text-slate-600">
          <span className="inline-flex items-center gap-1.5">
            <Sparkles className="h-3 w-3 text-acid-500/70" />
            Live Hugging Face Hub metadata and GGUF file sizes
          </span>
          <span>
            {loading ? "Searching…" : `${results.length} models`}
          </span>
        </div>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto px-6 py-5">
        {error ? (
          <div className="mb-4 flex items-start gap-2 rounded-xl border border-rose-400/20 bg-rose-500/5 px-4 py-3 text-[11px] leading-5 text-rose-300">
            <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
            <div>
              <p className="font-medium">Could not reach the Hub.</p>
              <p className="mt-0.5 text-rose-300/70">{error}</p>
            </div>
          </div>
        ) : null}

        {loading ? (
          <div className="grid gap-4 md:grid-cols-2 2xl:grid-cols-3">
            {Array.from({ length: 6 }).map((_, index) => (
              <div
                key={index}
                className="h-[232px] animate-pulse rounded-2xl border border-white/5 bg-ink-900/60"
              />
            ))}
          </div>
        ) : results.length ? (
          <div className="grid gap-4 md:grid-cols-2 2xl:grid-cols-3">
            {results.map((model) => (
              <ModelCard
                key={model.id}
                model={model}
                download={activeDownload(model.id)}
                onOpen={setSelected}
                onCancel={(id) => void onCancel(id)}
              />
            ))}
          </div>
        ) : (
          <div className="flex flex-col items-center py-20 text-center">
            <Search className="h-7 w-7 text-slate-700" />
            <p className="mt-3 text-sm font-medium text-slate-300">
              No models found
            </p>
            <p className="mt-1 max-w-sm text-[11px] leading-5 text-slate-600">
              Try a different search, or turn off “GGUF only” to see the full
              Hub catalog. Only GGUF repositories can be converted to .jns.
            </p>
          </div>
        )}
      </div>

      {selected ? (
        <DownloadDialog
          model={selected}
          modelsDir={settings?.models_dir ?? "models"}
          downloads={downloads}
          onClose={() => setSelected(null)}
          onDownload={(quant) => downloadQuant(selected, quant)}
          onPause={(id) => void onPause(id)}
          onResume={(id) => void onResume(id)}
          onCancel={(id) => void onCancel(id)}
        />
      ) : null}
    </div>
  );
}
