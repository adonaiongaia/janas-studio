import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  AlertTriangle,
  CheckCircle2,
  Cpu,
  Download,
  HardDrive,
  RefreshCw,
} from "lucide-react";

import { api, Events } from "../../lib/api";
import { cn } from "../../lib/cn";
import { errorMessage } from "../../lib/format";
import type {
  SetupBinary,
  SetupReport,
  SetupStatus,
} from "../../types";
import { Button } from "../ui/Button";

const STATUS_STYLES: Record<SetupStatus, string> = {
  idle: "border-white/10 bg-white/5 text-slate-400",
  running: "border-sky-400/20 bg-sky-400/5 text-sky-300",
  ready: "border-emerald-400/20 bg-emerald-400/5 text-emerald-300",
  degraded: "border-amber-400/20 bg-amber-400/5 text-amber-300",
  failed: "border-rose-400/20 bg-rose-400/5 text-rose-300",
};

function componentTone(status: SetupBinary["status"]): string {
  switch (status) {
    case "installed":
    case "present":
      return "text-emerald-300";
    case "failed":
      return "text-rose-300";
    case "skipped":
      return "text-amber-300";
    default:
      return "text-slate-400";
  }
}

function backendLabel(backend: string) {
  switch (backend) {
    case "cuda":
      return "NVIDIA CUDA";
    case "rocm":
      return "AMD ROCm";
    case "vulkan":
      return "Vulkan";
    case "metal":
      return "Apple Metal";
    default:
      return "CPU";
  }
}

export function SetupPanel() {
  const [report, setReport] = useState<SetupReport | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    void api
      .getEngineSetupStatus()
      .then((next) => {
        if (!disposed) setReport(next);
      })
      .catch((err) => {
        if (!disposed) setError(errorMessage(err));
      });

    void listen<SetupReport>(Events.engineSetupStatus, (event) => {
      if (!disposed) {
        setReport(event.payload);
        setBusy(false);
      }
    })
      .then((stop) => {
        if (disposed) stop();
        else unlisten = stop;
      })
      .catch(() => undefined);

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const rerun = async () => {
    setBusy(true);
    setError(null);
    try {
      setReport(await api.rerunEngineSetup());
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const status = report?.status ?? "idle";
  const platform = report?.platform;
  const gpuLabel = platform?.gpus.length
    ? platform.gpus
        .map((gpu) => `${gpu.name} (${backendLabel(gpu.backend)})`)
        .join(", ")
    : "No GPU detected";

  return (
    <section className="panel p-5">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h3 className="text-[12px] font-semibold text-white">
            Automatic engine setup
          </h3>
          <p className="mt-1 text-[10px] leading-4 text-slate-600">
            Runs silently at startup. Only missing managed binaries are fetched;
            an existing working binary is never replaced without a manual retry.
          </p>
        </div>
        <div className="flex items-center gap-2">
          <span
            className={cn(
              "inline-flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-[9px] font-medium uppercase tracking-wider",
              STATUS_STYLES[status],
            )}
          >
            {status === "ready" ? (
              <CheckCircle2 className="h-3 w-3" />
            ) : status === "failed" || status === "degraded" ? (
              <AlertTriangle className="h-3 w-3" />
            ) : (
              <Download className={cn("h-3 w-3", status === "running" && "animate-pulse")} />
            )}
            {status}
          </span>
          <Button
            size="sm"
            variant="secondary"
            busy={busy || status === "running"}
            onClick={() => void rerun()}
            icon={<RefreshCw className="h-3.5 w-3.5" />}
          >
            Retry setup
          </Button>
        </div>
      </div>

      {error ? (
        <div className="mt-3 flex items-start gap-2 rounded-lg border border-rose-400/20 bg-rose-500/5 px-3 py-2 text-[10px] leading-4 text-rose-300">
          <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
          {error}
        </div>
      ) : null}

      {report ? (
        <>
          <p className="mt-3 text-[10.5px] leading-5 text-slate-400">
            {report.message}
          </p>

          {platform ? (
            <div className="mt-3 grid gap-2 text-[10px] sm:grid-cols-2">
              <div className="flex items-start gap-2 rounded-lg border border-white/5 bg-white/[0.02] px-2.5 py-2">
                <Cpu className="mt-0.5 h-3.5 w-3.5 shrink-0 text-slate-500" />
                <div className="min-w-0">
                  <p className="text-slate-300">
                    {platform.os} · {platform.arch}
                  </p>
                  <p className="mt-0.5 truncate text-[9px] text-slate-600">
                    {platform.cpu_avx2 ? "AVX2 available" : "AVX2 unavailable"} ·{" "}
                    {gpuLabel}
                  </p>
                  <p className="mt-0.5 text-[9px] text-slate-600">
                    Preferred build: {backendLabel(platform.preferred_backend)}
                  </p>
                </div>
              </div>
              <div className="flex items-start gap-2 rounded-lg border border-white/5 bg-white/[0.02] px-2.5 py-2">
                <HardDrive className="mt-0.5 h-3.5 w-3.5 shrink-0 text-slate-500" />
                <div className="min-w-0">
                  <p className="text-slate-300">Janas cache</p>
                  <p className="mt-0.5 truncate font-mono text-[9px] text-slate-600">
                    {report.cache_dir ?? "not ready"}
                  </p>
                  {report.llama_release ? (
                    <p className="mt-0.5 truncate text-[9px] text-slate-600">
                      llama.cpp {report.llama_release.tag}
                      {report.llama_release.fallback ? " (nightly fallback)" : ""}
                    </p>
                  ) : null}
                </div>
              </div>
            </div>
          ) : null}

          <div className="mt-3 space-y-1.5">
            {report.binaries.map((binary) => (
              <div
                key={binary.name}
                className="flex items-start justify-between gap-3 rounded-lg border border-white/5 bg-white/[0.02] px-2.5 py-2"
              >
                <div className="min-w-0">
                  <p className="font-mono text-[10px] text-slate-300">
                    {binary.name}
                  </p>
                  <p className="mt-0.5 text-[9px] leading-4 text-slate-600">
                    {binary.message}
                  </p>
                  {binary.path ? (
                    <p className="mt-0.5 truncate font-mono text-[8.5px] text-slate-700">
                      {binary.path}
                      {binary.version ? ` · ${binary.version}` : ""}
                    </p>
                  ) : null}
                </div>
                <span
                  className={cn(
                    "shrink-0 text-[9px] font-medium uppercase tracking-wider",
                    componentTone(binary.status),
                  )}
                >
                  {binary.status}
                </span>
              </div>
            ))}
          </div>

          {report.warnings.length ? (
            <details className="mt-3 rounded-lg border border-amber-400/10 bg-amber-400/[0.02] px-2.5 py-2 text-[9px] leading-4 text-amber-200/70">
              <summary className="cursor-pointer select-none font-medium text-amber-200">
                {report.warnings.length} setup warning
                {report.warnings.length === 1 ? "" : "s"}
              </summary>
              <ul className="mt-1.5 space-y-1">
                {report.warnings.map((warning, index) => (
                  <li key={`${index}-${warning}`}>{warning}</li>
                ))}
              </ul>
            </details>
          ) : null}
        </>
      ) : (
        <p className="mt-3 text-[10px] text-slate-600">
          Checking hardware and managed binaries…
        </p>
      )}
    </section>
  );
}
