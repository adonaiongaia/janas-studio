import { useEffect, useState, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import {
  AlertTriangle,
  CheckCircle2,
  Download,
  FolderOpen,
  RefreshCw,
  RotateCcw,
  Save,
  Search,
  Server,
  ShieldCheck,
  Terminal,
} from "lucide-react";

import type { EngineController } from "../../hooks/useEngine";
import { useSettings } from "../../hooks/useSettings";
import { api, Events } from "../../lib/api";
import { cn } from "../../lib/cn";
import { errorMessage, formatBytes } from "../../lib/format";
import { DEFAULT_APP_SETTINGS } from "../../lib/settingsDefaults";
import type {
  AppSettings,
  BinaryInstallResult,
  BinaryProbe,
  BinaryProgress,
  LocalModel,
  ManagedBinaryProbe,
} from "../../types";
import { Button } from "../ui/Button";
import { GpuOffloadControl } from "../ui/GpuOffloadControl";
import { SetupPanel } from "./SetupPanel";
import { StatusPill } from "../ui/StatusPill";

interface SettingsViewProps {
  engine: EngineController;
}

const LLAMA_KV_OPTIONS = [
  "f16",
  "q8_0",
  "q5_1",
  "q5_0",
  "q4_1",
  "q4_0",
  "iq4_nl",
  "f32",
];

export function SettingsView({ engine }: SettingsViewProps) {
  const { settings, loading, save } = useSettings();
  const [form, setForm] = useState<AppSettings | null>(null);
  const [models, setModels] = useState<LocalModel[]>([]);
  const [probe, setProbe] = useState<BinaryProbe | null>(null);
  const [probing, setProbing] = useState(false);
  const [saving, setSaving] = useState(false);
  const [savedAt, setSavedAt] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [llamaInstallUrl, setLlamaInstallUrl] = useState("");
  const [llamaBusy, setLlamaBusy] = useState(false);
  const [llamaProbe, setLlamaProbe] = useState<ManagedBinaryProbe | null>(null);
  const [llamaInstall, setLlamaInstall] =
    useState<BinaryInstallResult | null>(null);
  const [llamaProgress, setLlamaProgress] =
    useState<BinaryProgress | null>(null);

  useEffect(() => {
    setForm(settings);
  }, [settings]);

  useEffect(() => {
    if (!savedAt) return;
    const timer = window.setTimeout(() => setSavedAt(null), 3_000);
    return () => window.clearTimeout(timer);
  }, [savedAt]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<BinaryProgress>(Events.engineBinaryProgress, (event) => {
      if (event.payload.engine === "llama") setLlamaProgress(event.payload);
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

  const loadModels = async () => {
    try {
      setModels(await api.scanLocalModels());
    } catch {
      setModels([]);
    }
  };

  useEffect(() => {
    void loadModels();
  }, [settings?.models_dir, settings?.model_path]);

  function update<K extends keyof AppSettings>(key: K, value: AppSettings[K]) {
    setForm((previous) => (previous ? { ...previous, [key]: value } : previous));
  }

  const dirty =
    Boolean(settings && form) &&
    JSON.stringify(settings) !== JSON.stringify(form);

  const pickFile = async (
    key: "engine_path" | "converter_path" | "model_path",
  ) => {
    if (!form) return;
    try {
      const picked = await open({
        multiple: false,
        directory: false,
        filters:
          key === "model_path"
            ? [{ name: "Janas model", extensions: ["jns"] }]
            : undefined,
      });
      if (typeof picked === "string") update(key, picked);
    } catch (err) {
      setError(errorMessage(err));
    }
  };

  const pickDirectory = async () => {
    try {
      const picked = await open({ multiple: false, directory: true });
      if (typeof picked === "string") update("models_dir", picked);
    } catch (err) {
      setError(errorMessage(err));
    }
  };

  const probeEngine = async () => {
    if (!form) return;
    setProbing(true);
    setProbe(null);
    setError(null);
    try {
      setProbe(await api.probeBinary(form.engine_path));
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setProbing(false);
    }
  };

  const pickLlamaFile = async (
    key: "llama_model_path" | "llama_mmproj_path",
  ) => {
    try {
      const picked = await open({
        multiple: false,
        directory: false,
        filters: [{ name: "GGUF file", extensions: ["gguf"] }],
      });
      if (typeof picked === "string") update(key, picked);
    } catch (err) {
      setError(errorMessage(err));
    }
  };

  const installLlama = async () => {
    if (!llamaInstallUrl.trim()) {
      setError(
        "Paste a direct llama.cpp release archive URL first. It must point at a .zip, .tar.gz, or single executable.",
      );
      return;
    }
    setLlamaBusy(true);
    setError(null);
    setLlamaInstall(null);
    setLlamaProbe(null);
    try {
      setLlamaInstall(
        await api.downloadAndExtractBinary("llama", llamaInstallUrl.trim()),
      );
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setLlamaBusy(false);
    }
  };

  const rollbackLlama = async () => {
    setLlamaBusy(true);
    setError(null);
    setLlamaInstall(null);
    try {
      setLlamaInstall(await api.rollbackBinary("llama"));
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setLlamaBusy(false);
    }
  };

  const probeLlama = async () => {
    setLlamaBusy(true);
    setError(null);
    try {
      setLlamaProbe(await api.probeManagedBinary("llama"));
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setLlamaBusy(false);
    }
  };

  const restartNote =
    engine.active?.kind === "llama"
      ? "llama.cpp is running. Changing the GGUF model, vision projector, port, or binary needs an engine restart."
      : "janas-server is running. Changing the model, port, or binary needs an engine restart.";

  const submit = async () => {
    if (!form) return;
    setSaving(true);
    setError(null);
    try {
      await save(form);
      setSavedAt(Date.now());
      await loadModels();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setSaving(false);
    }
  };

  const resetDefaults = () => {
    if (!form) return;
    const confirmed = window.confirm(
      "Reset all settings to their defaults?\n\nThis clears binary and model paths, the local API key, ports and runtime flags in the form. Your models folder is kept. Nothing is saved until you click Save changes.",
    );
    if (!confirmed) return;
    setForm({
      ...DEFAULT_APP_SETTINGS,
      models_dir: form.models_dir,
    });
    setSavedAt(null);
    setError(null);
  };

  const restart = async () => {
    await engine.stop();
    await engine.start();
  };

  if (loading || !form) {
    return (
      <div className="flex h-full items-center justify-center text-xs text-slate-500">
        Loading settings…
      </div>
    );
  }

  return (
    <div className="h-full overflow-y-auto">
      <div className="mx-auto max-w-4xl space-y-4 px-6 py-5">
        <div className="flex items-center justify-between gap-3">
          <div>
            <h2 className="text-sm font-semibold text-white">
              Runtime configuration
            </h2>
            <p className="mt-1 text-[11px] text-slate-500">
              Stored locally in the app config directory. No telemetry.
            </p>
          </div>
          <div className="flex items-center gap-2">
            {savedAt ? (
              <span className="inline-flex items-center gap-1 text-[10px] text-emerald-400">
                <CheckCircle2 className="h-3 w-3" />
                Saved
              </span>
            ) : null}
            <Button
              variant="secondary"
              size="sm"
              disabled={saving}
              onClick={resetDefaults}
              title="Load default settings into this form; Save changes to apply"
              icon={<RotateCcw className="h-3.5 w-3.5" />}
            >
              Reset defaults
            </Button>
            <Button
              variant="primary"
              size="sm"
              busy={saving}
              disabled={!dirty}
              onClick={() => void submit()}
              icon={<Save className="h-3.5 w-3.5" />}
            >
              Save changes
            </Button>
          </div>
        </div>

        {error ? (
          <div className="flex items-start gap-2 rounded-xl border border-rose-400/20 bg-rose-500/5 px-4 py-3 text-[11px] leading-5 text-rose-300">
            <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
            {error}
          </div>
        ) : null}

        {engine.snapshot.pid !== null ? (
          <div className="flex flex-wrap items-center gap-3 rounded-xl border border-sky-400/15 bg-sky-400/5 px-4 py-3">
            <StatusPill status={engine.snapshot.status} />
            <p className="min-w-0 flex-1 text-[10px] leading-4 text-sky-200/70">
              {restartNote}
            </p>
            <Button
              size="sm"
              variant="secondary"
              busy={engine.busy}
              onClick={() => void restart()}
              icon={<RefreshCw className="h-3 w-3" />}
            >
              Restart engine
            </Button>
          </div>
        ) : null}

        <SetupPanel />

        <Section
          title="Inference engine"
          description="Where the studio finds janas-server and the converter."
        >
          <Row
            label="janas-server binary"
            hint="A full path, or a name resolved on PATH."
          >
            <div className="flex gap-2">
              <input
                value={form.engine_path}
                onChange={(event) => update("engine_path", event.target.value)}
                className="field flex-1 font-mono text-[11px]"
                spellCheck={false}
              />
              <Button
                size="icon"
                variant="secondary"
                className="h-10 w-10"
                onClick={() => void pickFile("engine_path")}
                aria-label="Browse for janas-server"
                icon={<FolderOpen className="h-4 w-4" />}
              />
              <Button
                size="icon"
                variant="secondary"
                className="h-10 w-10"
                busy={probing}
                onClick={() => void probeEngine()}
                aria-label="Probe janas-server"
                icon={<Search className="h-4 w-4" />}
              />
            </div>
            {probe ? (
              <div
                className={cn(
                  "mt-2 rounded-lg border px-2.5 py-2 text-[10px] leading-4",
                  probe.ok
                    ? "border-emerald-400/20 bg-emerald-400/5 text-emerald-200"
                    : "border-rose-400/20 bg-rose-500/5 text-rose-300",
                )}
              >
                <p className="font-medium">{probe.message}</p>
                {probe.resolved_path ? (
                  <p className="mt-0.5 truncate font-mono text-[9px] opacity-70">
                    {probe.resolved_path}
                    {probe.version ? ` · ${probe.version}` : ""}
                  </p>
                ) : null}
              </div>
            ) : null}
          </Row>

          <Row
            label="Converter binary"
            hint="janas-convert with --input/--output, or upstream's gguf2jns."
          >
            <div className="flex gap-2">
              <input
                value={form.converter_path}
                onChange={(event) =>
                  update("converter_path", event.target.value)
                }
                className="field flex-1 font-mono text-[11px]"
                spellCheck={false}
              />
              <Button
                size="icon"
                variant="secondary"
                className="h-10 w-10"
                onClick={() => void pickFile("converter_path")}
                aria-label="Browse for the converter"
                icon={<FolderOpen className="h-4 w-4" />}
              />
            </div>
            <p className="mt-1.5 text-[9.5px] leading-4 text-slate-600">
              If this is left as{" "}
              <span className="font-mono">janas-convert</span> and not found, the
              studio also looks for <span className="font-mono">gguf2jns</span>.
            </p>
          </Row>
        </Section>

        <Section
          title="llama.cpp engine"
          description="A second, runtime-managed engine. Its binary lives in the app's local data bin folder, never inside the read-only AppImage mount."
        >
          <Row
            label="GGUF model"
            hint="Passed as llama-server --model. Choose a .gguf file on this machine."
          >
            <div className="flex gap-2">
              <input
                value={form.llama_model_path}
                onChange={(event) =>
                  update("llama_model_path", event.target.value)
                }
                placeholder="/models/qwen3-4b-q4_k_m.gguf"
                className="field flex-1 font-mono text-[11px]"
                spellCheck={false}
              />
              <Button
                size="icon"
                variant="secondary"
                className="h-10 w-10"
                onClick={() => void pickLlamaFile("llama_model_path")}
                aria-label="Browse for a GGUF model"
                icon={<FolderOpen className="h-4 w-4" />}
              />
            </div>
          </Row>

          <Row
            label="Vision projector"
            hint="Optional --mmproj GGUF for image attachments. Choose the projector that matches the selected model, then restart llama.cpp."
          >
            <div className="flex gap-2">
              <input
                value={form.llama_mmproj_path}
                onChange={(event) =>
                  update("llama_mmproj_path", event.target.value)
                }
                placeholder="/models/mmproj-model-f16.gguf"
                className="field flex-1 font-mono text-[11px]"
                spellCheck={false}
              />
              <Button
                size="icon"
                variant="secondary"
                className="h-10 w-10"
                onClick={() => void pickLlamaFile("llama_mmproj_path")}
                aria-label="Browse for a vision projector GGUF"
                icon={<FolderOpen className="h-4 w-4" />}
              />
            </div>
            <p className="mt-1.5 text-[9.5px] leading-4 text-slate-600">
              Leave empty for text-only models. Image input needs both this
              projector and a vision-capable GGUF model.
            </p>
          </Row>

          <Row
            label="Managed binary"
            hint="Paste a direct llama.cpp release asset URL. ZIP, tar.gz and single executables are supported; the installer verifies the binary before keeping one .bak."
          >
            <div className="flex flex-col gap-2 sm:flex-row">
              <input
                value={llamaInstallUrl}
                onChange={(event) => setLlamaInstallUrl(event.target.value)}
                placeholder="https://github.com/ggml-org/llama.cpp/releases/download/.../llama-bin-ubuntu-x64.zip"
                className="field min-w-0 flex-1 font-mono text-[10px]"
                spellCheck={false}
              />
              <Button
                size="sm"
                busy={llamaBusy}
                disabled={!llamaInstallUrl.trim()}
                onClick={() => void installLlama()}
                icon={<Download className="h-3.5 w-3.5" />}
              >
                Install
              </Button>
            </div>
            <div className="mt-2 flex flex-wrap gap-2">
              <Button
                size="sm"
                variant="secondary"
                busy={llamaBusy}
                onClick={() => void rollbackLlama()}
                icon={<RotateCcw className="h-3.5 w-3.5" />}
              >
                Roll back to .bak
              </Button>
              <Button
                size="sm"
                variant="secondary"
                busy={llamaBusy}
                onClick={() => void probeLlama()}
                icon={<ShieldCheck className="h-3.5 w-3.5" />}
              >
                Probe managed binary
              </Button>
            </div>

            {llamaProgress ? (
              <div className="mt-2 rounded-lg border border-white/10 bg-white/[0.02] px-2.5 py-2">
                <div className="flex items-center justify-between gap-2 text-[10px]">
                  <span className="text-slate-300">{llamaProgress.message}</span>
                  <span className="shrink-0 font-mono text-[9px] uppercase text-slate-500">
                    {llamaProgress.percent !== null
                      ? `${Math.round(llamaProgress.percent)}%`
                      : llamaProgress.phase}
                  </span>
                </div>
                {llamaProgress.percent !== null ? (
                  <div className="mt-2 h-1 overflow-hidden rounded-full bg-white/10">
                    <div
                      className="h-full rounded-full bg-acid-400 transition-all"
                      style={{ width: `${llamaProgress.percent}%` }}
                    />
                  </div>
                ) : null}
              </div>
            ) : null}

            {llamaProbe ? (
              <div
                className={cn(
                  "mt-2 rounded-lg border px-2.5 py-2 text-[10px] leading-4",
                  llamaProbe.installed
                    ? "border-emerald-400/20 bg-emerald-400/5 text-emerald-200"
                    : "border-amber-400/20 bg-amber-400/5 text-amber-200",
                )}
              >
                <p className="font-medium">{llamaProbe.message}</p>
                <p className="mt-0.5 truncate font-mono text-[9px] opacity-70">
                  {llamaProbe.path}
                  {llamaProbe.version ? ` · ${llamaProbe.version}` : ""}
                </p>
              </div>
            ) : null}

            {llamaInstall ? (
              <div className="mt-2 rounded-lg border border-emerald-400/20 bg-emerald-400/5 px-2.5 py-2 text-[10px] leading-4 text-emerald-200">
                <p className="flex items-center gap-1 font-medium">
                  <CheckCircle2 className="h-3 w-3" />
                  {llamaInstall.message}
                </p>
                <p className="mt-0.5 truncate font-mono text-[9px] opacity-70">
                  {llamaInstall.installed_path}
                </p>
              </div>
            ) : null}
          </Row>

          <Row
            label="llama-server port"
            hint="Loopback port for the llama engine. Janas keeps its own port."
          >
            <input
              type="number"
              min={1}
              max={65535}
              value={form.llama_port}
              onChange={(event) =>
                update("llama_port", Number(event.target.value) || 0)
              }
              className="field font-mono text-[11px]"
            />
          </Row>

          <Row
            label="GPU offload"
            hint="--n-gpu-layers: 0 is CPU-only. The maximum follows the selected GGUF's layer count."
          >
            <GpuOffloadControl
              modelPath={form.llama_model_path}
              value={form.llama_gpu_layers}
              onChange={(value) => update("llama_gpu_layers", value)}
            />
          </Row>

          <Row
            label="Context size"
            hint="-c / --ctx-size: the token window llama-server allocates."
          >
            <input
              type="number"
              min={512}
              step={512}
              value={form.llama_ctx_size}
              onChange={(event) =>
                update("llama_ctx_size", Number(event.target.value) || 512)
              }
              className="field font-mono text-[11px]"
            />
          </Row>

          <Row
            label="Flash attention"
            hint="--flash-attn: auto lets the build decide; off is the compatibility fallback."
          >
            <select
              value={form.llama_flash_attn}
              onChange={(event) =>
                update("llama_flash_attn", event.target.value)
              }
              className="field font-mono text-[11px]"
            >
              <option value="auto">auto</option>
              <option value="on">on</option>
              <option value="off">off</option>
            </select>
          </Row>

          <Row
            label="KV cache quantization"
            hint="--cache-type-k and --cache-type-v. Lower precision saves memory; f16 is the safe default."
          >
            <div className="grid grid-cols-2 gap-2">
              <label className="space-y-1">
                <span className="text-[9px] uppercase tracking-wider text-slate-600">
                  K cache
                </span>
                <select
                  value={form.llama_kv_cache_k}
                  onChange={(event) =>
                    update("llama_kv_cache_k", event.target.value)
                  }
                  className="field font-mono text-[11px]"
                >
                  {LLAMA_KV_OPTIONS.map((value) => (
                    <option key={value} value={value}>
                      {value}
                    </option>
                  ))}
                </select>
              </label>
              <label className="space-y-1">
                <span className="text-[9px] uppercase tracking-wider text-slate-600">
                  V cache
                </span>
                <select
                  value={form.llama_kv_cache_v}
                  onChange={(event) =>
                    update("llama_kv_cache_v", event.target.value)
                  }
                  className="field font-mono text-[11px]"
                >
                  {LLAMA_KV_OPTIONS.map((value) => (
                    <option key={value} value={value}>
                      {value}
                    </option>
                  ))}
                </select>
              </label>
            </div>
          </Row>

          <Row
            label="Extra llama arguments"
            hint="Parsed with shell-word rules and appended last. Use --moe-cache-mib or --threads here to override Direct CPU MoE defaults."
          >
            <div className="relative">
              <Terminal className="pointer-events-none absolute left-3 top-3.5 h-3.5 w-3.5 text-slate-600" />
              <input
                value={form.llama_extra_args}
                onChange={(event) =>
                  update("llama_extra_args", event.target.value)
                }
                placeholder="--moe-cache-mib 4096 --threads 8"
                className="field pl-9 font-mono text-[11px]"
                spellCheck={false}
              />
            </div>
          </Row>
        </Section>

        <Section
          title="Model and server"
          description="The .jns file janas-server opens, and the loopback API it exposes."
        >
          <Row label="Active model" hint="Only converted .jns models are runnable.">
            <div className="flex gap-2">
              <input
                value={form.model_path}
                onChange={(event) => update("model_path", event.target.value)}
                placeholder="/models/qwen3-4b.jns"
                className="field flex-1 font-mono text-[11px]"
                spellCheck={false}
              />
              <Button
                size="icon"
                variant="secondary"
                className="h-10 w-10"
                onClick={() => void pickFile("model_path")}
                aria-label="Browse for a .jns model"
                icon={<FolderOpen className="h-4 w-4" />}
              />
            </div>

            <div className="mt-2.5 space-y-1.5">
              {models.length ? (
                <>
                  <p className="text-[9px] uppercase tracking-wider text-slate-600">
                    Found in your models folder
                  </p>
                  {models.slice(0, 10).map((model) => (
                    <div
                      key={model.path}
                      className="flex items-center justify-between gap-2 rounded-lg border border-white/5 bg-white/[0.02] px-2.5 py-1.5"
                    >
                      <div className="min-w-0">
                        <p className="truncate text-[10.5px] text-slate-300">
                          {model.name}
                        </p>
                        <p className="text-[9px] text-slate-600">
                          {formatBytes(model.size_bytes)}
                        </p>
                      </div>
                      <button
                        type="button"
                        onClick={() => update("model_path", model.path)}
                        className={cn(
                          "shrink-0 rounded-md px-2 py-1 text-[9px] font-medium transition",
                          form.model_path === model.path
                            ? "bg-acid-400/15 text-acid-300"
                            : "bg-white/5 text-slate-400 hover:bg-white/10 hover:text-slate-200",
                        )}
                      >
                        {form.model_path === model.path ? "Selected" : "Use"}
                      </button>
                    </div>
                  ))}
                </>
              ) : (
                <p className="rounded-lg border border-white/5 bg-white/[0.02] px-2.5 py-2 text-[10px] text-slate-600">
                  No .jns files found yet. Download a GGUF in Models and compile
                  it in the Compiler tab.
                </p>
              )}
            </div>
          </Row>

          <Row label="Models folder" hint="Downloads and converted models land here.">
            <div className="flex gap-2">
              <input
                value={form.models_dir}
                onChange={(event) => update("models_dir", event.target.value)}
                className="field flex-1 font-mono text-[11px]"
                spellCheck={false}
              />
              <Button
                size="icon"
                variant="secondary"
                className="h-10 w-10"
                onClick={() => void pickDirectory()}
                aria-label="Browse for a models folder"
                icon={<FolderOpen className="h-4 w-4" />}
              />
            </div>
          </Row>

          <Row
            label="Server address"
            hint="Keep the host on 127.0.0.1 unless you intend to expose the engine."
          >
            <div className="grid grid-cols-[1fr_120px] gap-2">
              <div className="relative">
                <Server className="pointer-events-none absolute left-3 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-slate-600" />
                <input
                  value={form.host}
                  onChange={(event) => update("host", event.target.value)}
                  className="field pl-9 font-mono text-[11px]"
                  spellCheck={false}
                />
              </div>
              <input
                type="number"
                min={1}
                max={65535}
                value={form.port}
                onChange={(event) =>
                  update("port", Number(event.target.value) || 0)
                }
                className="field font-mono text-[11px]"
              />
            </div>
          </Row>

          <Row
            label="Flash attention"
            hint="JANAS_ATTN: auto lets Janas choose; exact keeps float attention; fast uses a 16-bit query for faster scores."
          >
            <select
              value={form.janas_attention}
              onChange={(event) =>
                update("janas_attention", event.target.value)
              }
              className="field font-mono text-[11px]"
            >
              <option value="auto">auto</option>
              <option value="exact">exact</option>
              <option value="fast">fast</option>
            </select>
          </Row>

          <Row
            label="KV cache quantization"
            hint="JANAS_KV: applies to both K and V. Auto follows the model; Gemma 4 uses 16-bit and Qwen uses 8-bit by default."
          >
            <select
              value={form.janas_kv_cache}
              onChange={(event) => update("janas_kv_cache", event.target.value)}
              className="field font-mono text-[11px]"
            >
              <option value="auto">auto</option>
              <option value="8">8-bit</option>
              <option value="16">16-bit</option>
            </select>
          </Row>

          <Row
            label="Extra arguments"
            hint="Passed to janas-server verbatim, for example --ctx 8192 --no-gpu."
          >
            <div className="relative">
              <Terminal className="pointer-events-none absolute left-3 top-3.5 h-3.5 w-3.5 text-slate-600" />
              <input
                value={form.extra_args}
                onChange={(event) => update("extra_args", event.target.value)}
                placeholder="--ctx 8192 --cache 8"
                className="field pl-9 font-mono text-[11px]"
                spellCheck={false}
              />
            </div>
          </Row>

          <Row
            label="Local API key"
            hint="Optional. Passed as --api-key and used for chat requests."
          >
            <input
              type="password"
              value={form.api_key}
              onChange={(event) => update("api_key", event.target.value)}
              placeholder="none"
              className="field font-mono text-[11px]"
              spellCheck={false}
            />
          </Row>
        </Section>

        <section className="panel p-5">
          <h3 className="text-[12px] font-semibold text-white">
            About this build
          </h3>
          <div className="mt-3 space-y-2 text-[10.5px] leading-5 text-slate-500">
            <p>
              Janas Studio is a local front end for{" "}
              <span className="text-slate-300">janas-server</span>, the
              OpenAI-compatible HTTP API of the Janas C inference engine. Chat
              requests stream from{" "}
              <span className="font-mono text-slate-400">
                /v1/chat/completions
              </span>{" "}
              through the Rust process manager, so the webview never needs
              cross-origin access to the engine.
            </p>
            <p>
              GGUF models are downloaded from the Hugging Face Hub into your
              models folder, then rewritten into Janas's{" "}
              <span className="text-slate-300">.jns</span> format by the
              converter. Conversion is not re-quantization; the engine reads the
              converted file directly.
            </p>
            <p>
              No model weights are bundled with the app. An AppImage build
              requires WebKitGTK 4.1 on the host, as is standard for Tauri on
              Linux.
            </p>
          </div>
        </section>
      </div>
    </div>
  );
}

function Section({
  title,
  description,
  children,
}: {
  title: string;
  description: string;
  children: ReactNode;
}) {
  return (
    <section className="panel p-5">
      <h3 className="text-[12px] font-semibold text-white">{title}</h3>
      <p className="mt-1 text-[10px] leading-4 text-slate-600">{description}</p>
      <div className="mt-4 space-y-4">{children}</div>
    </section>
  );
}

function Row({
  label,
  hint,
  children,
}: {
  label: string;
  hint: string;
  children: ReactNode;
}) {
  return (
    <div className="grid gap-2 border-t border-white/5 pt-4 first:border-0 first:pt-0 md:grid-cols-[180px_minmax(0,1fr)] md:gap-5">
      <div>
        <p className="text-[11px] font-medium text-slate-300">{label}</p>
        <p className="mt-0.5 text-[9.5px] leading-4 text-slate-600">{hint}</p>
      </div>
      <div className="min-w-0">{children}</div>
    </div>
  );
}
