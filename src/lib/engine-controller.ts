import { api } from "./api";
import type {
  ActiveEngineInfo,
  AppSettings,
  EngineKind,
  EnginePreferences,
  EngineSnapshot,
} from "../types";

export const ENGINE_LABELS: Record<EngineKind, string> = {
  janas: "Janas",
  llama: "llama.cpp",
};

export function engineLabel(
  kind: EngineKind | null | undefined,
  fallback = "No engine",
): string {
  return kind ? ENGINE_LABELS[kind] : fallback;
}

/** The sampling block the Rust proxy forwards to either engine. Both engines
 * accept the OpenAI-compatible fields plus the integer `top_k`. */
export function samplingForRequest(preferences: EnginePreferences) {
  return {
    temperature: preferences.temperature,
    top_p: preferences.topP,
    top_k: Math.round(Math.min(100, Math.max(1, preferences.topK ?? 40))),
    min_p: preferences.minP,
    repeat_penalty: preferences.repeatPenalty,
  };
}

/** Runtime flags surfaced in the panel. The authoritative mapping lives in
 * Rust; this preview keeps the UI honest about what will be launched. */
export function llamaRuntimePreview(settings: AppSettings): string[] {
  const args = [
    "--n-gpu-layers",
    String(settings.llama_gpu_layers),
    "--ctx-size",
    String(settings.llama_ctx_size),
    "--cache-type-k",
    settings.llama_kv_cache_k,
    "--cache-type-v",
    settings.llama_kv_cache_v,
  ];
  if (settings.llama_flash_attn !== "auto") {
    args.push("--flash-attn", settings.llama_flash_attn);
  }
  if (settings.llama_mmproj_path.trim()) {
    args.push("--mmproj", settings.llama_mmproj_path.trim());
  }
  if (settings.llama_direct_cpu_moe) {
    args.push(
      "--cpu-moe",
      "--moe-cache-mib",
      "2048",
      "--moe-cache-cpu-share",
      "1.0",
    );
    if (settings.llama_cpu_threads > 0) {
      args.push("--threads", String(settings.llama_cpu_threads));
    }
  }
  return args;
}

export const engineController = {
  current: (): Promise<ActiveEngineInfo | null> => api.getActiveEngine(),
  currentStatus: (): Promise<EngineSnapshot> => api.getEngineStatus(),
  start: (): Promise<EngineSnapshot> => api.startEngine(),
  stop: (): Promise<void> => api.stopEngine(),
  switchTo: (kind: EngineKind | "none"): Promise<EngineSnapshot> =>
    api.switchEngine(kind),
  installBinary: (kind: EngineKind, downloadUrl: string) =>
    api.downloadAndExtractBinary(kind, downloadUrl),
  rollbackBinary: (kind: EngineKind) => api.rollbackBinary(kind),
  probeManaged: (kind: EngineKind) => api.probeManagedBinary(kind),
};
