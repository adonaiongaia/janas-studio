import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";
import {
  AlertTriangle,
  Binary,
  CheckCircle2,
  Copy,
  FileCode2,
  FolderOpen,
  Loader2,
  Play,
  Save,
  Square,
  Terminal,
  Trash2,
  UploadCloud,
  XCircle,
} from "lucide-react";

import type { ConverterController } from "../../hooks/useConverter";
import { useSettings } from "../../hooks/useSettings";
import { cn } from "../../lib/cn";
import {
  basename,
  errorMessage,
  formatBytes,
  formatDuration,
  stripExtension,
} from "../../lib/format";
import { Button } from "../ui/Button";
import { ProgressBar } from "../ui/ProgressBar";

interface ConverterViewProps {
  onUseModel: (path: string) => Promise<void>;
  converter: ConverterController;
}

export function ConverterView({
  onUseModel,
  converter,
}: ConverterViewProps) {
  const { settings } = useSettings();
  const [input, setInput] = useState("");
  const [output, setOutput] = useState("");
  const [overwrite, setOverwrite] = useState(false);
  const [dragging, setDragging] = useState(false);
  const [localError, setLocalError] = useState<string | null>(null);
  const [using, setUsing] = useState(false);
  const consoleRef = useRef<HTMLDivElement>(null);

  const chooseInput = useCallback(
    (path: string) => {
      setInput(path);
      setLocalError(null);
      if (settings) {
        setOutput(
          `${settings.models_dir.replace(/\/+$/, "")}/${stripExtension(
            basename(path),
          )}.jns`,
        );
      }
    },
    [settings],
  );

  useEffect(() => {
    let disposed = false;
    let unlisten: UnlistenFn | undefined;

    void getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type === "enter" || event.payload.type === "over") {
          setDragging(true);
          return;
        }
        setDragging(false);
        if (event.payload.type === "drop") {
          const path = event.payload.paths.find((candidate) =>
            candidate.toLowerCase().endsWith(".gguf"),
          );
          if (path) chooseInput(path);
          else setLocalError("That file is not a .gguf model.");
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
  }, [chooseInput]);

  useEffect(() => {
    const element = consoleRef.current;
    if (element) element.scrollTop = element.scrollHeight;
  }, [converter.logs]);

  const pickInput = async () => {
    try {
      const picked = await open({
        multiple: false,
        directory: false,
        filters: [{ name: "GGUF model", extensions: ["gguf"] }],
      });
      if (typeof picked === "string") chooseInput(picked);
    } catch (err) {
      setLocalError(errorMessage(err));
    }
  };

  const pickOutput = async () => {
    try {
      const picked = await save({
        defaultPath:
          output ||
          (settings?.models_dir
            ? `${settings.models_dir.replace(/\/+$/, "")}/model.jns`
            : undefined),
        filters: [{ name: "Janas model", extensions: ["jns"] }],
      });
      if (typeof picked === "string") setOutput(picked);
    } catch (err) {
      setLocalError(errorMessage(err));
    }
  };

  const run = async () => {
    setLocalError(null);
    if (!input.trim()) {
      setLocalError("Choose a .gguf file first.");
      return;
    }
    if (!output.trim()) {
      setLocalError("Choose where the .jns file should be written.");
      return;
    }
    await converter.start({
      input: input.trim(),
      output: output.trim(),
      overwrite,
    });
  };

  const useConvertedModel = async () => {
    if (!converter.finished?.success) return;
    setUsing(true);
    setLocalError(null);
    try {
      await onUseModel(converter.finished.output_path);
    } catch (err) {
      setLocalError(errorMessage(err));
    } finally {
      setUsing(false);
    }
  };

  const copyLogs = async () => {
    try {
      await navigator.clipboard.writeText(
        converter.logs.map((log) => log.line).join("\n"),
      );
    } catch {
      // Clipboard may be unavailable; logs remain selectable.
    }
  };

  const percent = converter.finished?.success
    ? 100
    : converter.progress;
  const statusText = converter.running
    ? converter.progress !== null
      ? `Converting · ${converter.progress.toFixed(0)}%`
      : "Converting · reading model…"
    : converter.finished
      ? converter.finished.message
      : "Idle — no conversion has run yet.";

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="grid min-h-0 flex-1 gap-4 p-5 lg:grid-cols-[390px_minmax(0,1fr)]">
        <div className="min-h-0 space-y-4 overflow-y-auto pr-1">
          <section className="panel p-4">
            <div className="flex items-center gap-2">
              <Binary className="h-4 w-4 text-acid-400" />
              <h2 className="text-[13px] font-semibold text-white">
                Source model
              </h2>
            </div>
            <p className="mt-1 text-[10px] leading-4 text-slate-500">
              Janas rewrites the weights it already understands. Pick a Q4_K_M,
              Q3_K, IQ4 or Q5 GGUF and compile it to .jns.
            </p>

            <div
              className={cn(
                "mt-4 flex flex-col items-center justify-center rounded-2xl border border-dashed px-4 py-7 text-center transition",
                dragging
                  ? "border-acid-400/70 bg-acid-400/5"
                  : "border-white/10 bg-white/[0.02]",
              )}
            >
              <UploadCloud
                className={cn(
                  "h-6 w-6",
                  dragging ? "text-acid-300" : "text-slate-500",
                )}
              />
              <p className="mt-2 text-[11px] font-medium text-slate-300">
                Drop a .gguf model here
              </p>
              <p className="mt-0.5 text-[10px] text-slate-600">
                or{" "}
                <button
                  type="button"
                  onClick={() => void pickInput()}
                  className="text-acid-300 underline decoration-acid-400/30 underline-offset-2 hover:text-acid-200"
                >
                  browse files
                </button>
              </p>
              {input ? (
                <p className="mt-3 max-w-full truncate rounded-md border border-white/10 bg-ink-950/60 px-2 py-1 font-mono text-[9px] text-slate-400">
                  {input}
                </p>
              ) : null}
            </div>

            <div className="mt-4 space-y-3">
              <div>
                <label htmlFor="convert-input" className="label">
                  Input path
                </label>
                <div className="mt-1.5 flex gap-2">
                  <input
                    id="convert-input"
                    value={input}
                    onChange={(event) => setInput(event.target.value)}
                    placeholder="/models/Qwen3-4B-Q4_K_M.gguf"
                    className="field flex-1 font-mono text-[10.5px]"
                    spellCheck={false}
                  />
                  <Button
                    size="icon"
                    variant="secondary"
                    onClick={() => void pickInput()}
                    aria-label="Browse for a GGUF file"
                    className="h-10 w-10"
                    icon={<FolderOpen className="h-4 w-4" />}
                  />
                </div>
              </div>

              <div>
                <label htmlFor="convert-output" className="label">
                  Output path
                </label>
                <div className="mt-1.5 flex gap-2">
                  <input
                    id="convert-output"
                    value={output}
                    onChange={(event) => setOutput(event.target.value)}
                    placeholder="/models/qwen3-4b.jns"
                    className="field flex-1 font-mono text-[10.5px]"
                    spellCheck={false}
                  />
                  <Button
                    size="icon"
                    variant="secondary"
                    onClick={() => void pickOutput()}
                    aria-label="Choose an output path"
                    className="h-10 w-10"
                    icon={<Save className="h-4 w-4" />}
                  />
                </div>
              </div>

              <label className="flex cursor-pointer select-none items-center gap-2 text-[11px] text-slate-400">
                <input
                  type="checkbox"
                  checked={overwrite}
                  onChange={(event) => setOverwrite(event.target.checked)}
                  className="h-3.5 w-3.5 rounded border-white/20 bg-ink-900 accent-acid-400"
                />
                Overwrite an existing .jns file
              </label>
            </div>

            {localError || converter.error ? (
              <div className="mt-4 flex items-start gap-2 rounded-xl border border-rose-400/20 bg-rose-500/5 px-3 py-2.5 text-[10px] leading-4 text-rose-300">
                <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
                <span>{localError ?? converter.error}</span>
              </div>
            ) : null}

            <div className="mt-4 flex gap-2">
              <Button
                className="flex-1 justify-center"
                variant="primary"
                disabled={converter.running}
                onClick={() => void run()}
                icon={<Play className="h-3.5 w-3.5" />}
              >
                Convert to .jns Format
              </Button>
              {converter.running ? (
                <Button
                  variant="danger"
                  onClick={() => void converter.cancel()}
                  icon={<Square className="h-3 w-3" />}
                >
                  Stop
                </Button>
              ) : null}
            </div>

            <p className="mt-3 text-[9px] leading-4 text-slate-600">
              The studio calls <span className="font-mono">janas-convert</span>{" "}
              with <span className="font-mono">--input/--output</span>; upstream
              Janas's <span className="font-mono">gguf2jns</span> is detected and
              called positionally.
            </p>
          </section>

          {converter.finished?.success ? (
            <section className="rounded-2xl border border-emerald-400/20 bg-emerald-400/5 p-4">
              <div className="flex items-start gap-2.5">
                <CheckCircle2 className="mt-0.5 h-4 w-4 shrink-0 text-emerald-400" />
                <div className="min-w-0">
                  <p className="text-[12px] font-medium text-emerald-200">
                    Compiled successfully
                  </p>
                  <p className="mt-1 truncate font-mono text-[9.5px] text-emerald-200/60">
                    {converter.finished.output_path}
                  </p>
                  <p className="mt-1 text-[10px] text-emerald-200/70">
                    {formatBytes(converter.finished.output_size)} ·{" "}
                    {formatDuration(converter.finished.elapsed_ms)}
                  </p>
                </div>
              </div>
              <Button
                size="sm"
                variant="primary"
                className="mt-3 w-full justify-center"
                busy={using}
                onClick={() => void useConvertedModel()}
                icon={<FileCode2 className="h-3.5 w-3.5" />}
              >
                Use this model in Chat
              </Button>
            </section>
          ) : null}

          {converter.finished && !converter.finished.success ? (
            <section className="rounded-2xl border border-rose-400/20 bg-rose-500/5 p-4">
              <div className="flex items-start gap-2.5">
                <XCircle className="mt-0.5 h-4 w-4 shrink-0 text-rose-400" />
                <div>
                  <p className="text-[12px] font-medium text-rose-200">
                    Conversion failed
                  </p>
                  <p className="mt-1 text-[10px] leading-4 text-rose-200/70">
                    {converter.finished.message} Review the console output for
                    the converter's error.
                  </p>
                </div>
              </div>
            </section>
          ) : null}
        </div>

        <section className="panel flex min-h-0 flex-col overflow-hidden">
          <div className="flex h-11 shrink-0 items-center gap-3 border-b border-white/5 px-4">
            <div className="flex gap-1.5">
              <span className="h-2.5 w-2.5 rounded-full bg-rose-400/60" />
              <span className="h-2.5 w-2.5 rounded-full bg-amber-400/60" />
              <span className="h-2.5 w-2.5 rounded-full bg-emerald-400/60" />
            </div>
            <div className="flex min-w-0 items-center gap-2 text-[10px] text-slate-500">
              <Terminal className="h-3.5 w-3.5" />
              <span className="truncate font-mono text-slate-400">
                {converter.started?.converter_path ?? "janas-convert"}
              </span>
            </div>
            <div className="ml-auto flex items-center gap-1">
              <Button
                size="icon"
                variant="ghost"
                onClick={() => void copyLogs()}
                disabled={!converter.logs.length}
                aria-label="Copy console output"
                icon={<Copy className="h-3.5 w-3.5" />}
              />
              <Button
                size="icon"
                variant="ghost"
                onClick={converter.clear}
                disabled={converter.running || !converter.logs.length}
                aria-label="Clear console"
                icon={<Trash2 className="h-3.5 w-3.5" />}
              />
            </div>
          </div>

          <div className="flex shrink-0 items-center gap-3 border-b border-white/5 px-4 py-3">
            <ProgressBar
              value={percent}
              indeterminate={converter.running && percent === null}
              className="flex-1"
            />
            <span className="w-10 shrink-0 text-right font-mono text-[10px] text-slate-400">
              {percent !== null
                ? `${percent.toFixed(0)}%`
                : converter.running
                  ? "—"
                  : "0%"}
            </span>
          </div>

          <div
            ref={consoleRef}
            role="log"
            aria-live="polite"
            className="min-h-0 flex-1 overflow-y-auto bg-ink-950/70 p-4 font-mono text-[11px] leading-5"
          >
            {converter.logs.length ? (
              converter.logs.map((log, index) => (
                <div
                  key={`${log.timestamp_ms}-${index}`}
                  className={cn(
                    "flex gap-2.5 whitespace-pre-wrap break-words",
                    log.stream === "system"
                      ? "text-acid-300/90"
                      : log.stream === "stderr"
                        ? "text-rose-300/80"
                        : /error|failed|refus/i.test(log.line)
                          ? "text-rose-300/90"
                          : "text-slate-400",
                  )}
                >
                  <span className="shrink-0 select-none text-slate-700">
                    {logTime(log.timestamp_ms)}
                  </span>
                  <span className="min-w-0">{log.line}</span>
                </div>
              ))
            ) : (
              <div className="flex h-full flex-col items-center justify-center text-center">
                <Terminal className="h-6 w-6 text-slate-700" />
                <p className="mt-2 text-[11px] text-slate-500">
                  Converter output appears here.
                </p>
                <p className="mt-1 max-w-xs text-[10px] leading-4 text-slate-700">
                  stdout and stderr are streamed line by line; percentages are
                  parsed into the progress bar.
                </p>
              </div>
            )}
          </div>

          <div className="flex h-9 shrink-0 items-center justify-between border-t border-white/5 px-4 text-[9.5px]">
            <span className="flex min-w-0 items-center gap-1.5">
              {converter.running ? (
                <Loader2 className="h-3 w-3 animate-spin text-acid-400" />
              ) : converter.finished?.success ? (
                <CheckCircle2 className="h-3 w-3 text-emerald-400" />
              ) : converter.finished ? (
                <XCircle className="h-3 w-3 text-rose-400" />
              ) : (
                <span className="h-1.5 w-1.5 rounded-full bg-slate-600" />
              )}
              <span className="truncate text-slate-500">{statusText}</span>
            </span>
            <span className="ml-3 shrink-0 font-mono text-slate-600">
              {converter.finished
                ? `${formatDuration(converter.finished.elapsed_ms)} · ${formatBytes(
                    converter.finished.output_size,
                  )}`
                : converter.started
                  ? "streaming"
                  : "idle"}
            </span>
          </div>
        </section>
      </div>
    </div>
  );
}

function logTime(timestamp: number): string {
  return new Date(timestamp).toLocaleTimeString("en-GB", { hour12: false });
}
