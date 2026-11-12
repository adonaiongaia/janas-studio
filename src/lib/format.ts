import type { HfModel } from "../types";

const BYTE_UNITS = ["B", "KB", "MB", "GB", "TB"];

export function formatBytes(bytes: number, digits = 1): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "—";
  let size = bytes;
  let unit = 0;
  while (size >= 1024 && unit < BYTE_UNITS.length - 1) {
    size /= 1024;
    unit += 1;
  }
  return `${size.toFixed(unit === 0 ? 0 : digits)} ${BYTE_UNITS[unit]}`;
}

export function formatCount(value: number): string {
  if (!Number.isFinite(value)) return "0";
  return new Intl.NumberFormat("en", {
    notation: "compact",
    maximumFractionDigits: 1,
  }).format(value);
}

export function formatParameters(value: number | null): string {
  if (!value || value <= 0) return "—";
  if (value >= 1_000_000_000) return `${(value / 1_000_000_000).toFixed(1)}B params`;
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(0)}M params`;
  return `${formatCount(value)} params`;
}

export function formatDuration(ms: number): string {
  if (ms < 1000) return `${Math.max(1, Math.round(ms))} ms`;
  const seconds = ms / 1000;
  if (seconds < 60) return `${seconds.toFixed(1)} s`;
  const minutes = Math.floor(seconds / 60);
  return `${minutes}m ${Math.round(seconds % 60)}s`;
}

export function formatRelativeDate(input: string | null): string {
  if (!input) return "—";
  const date = new Date(input);
  if (Number.isNaN(date.getTime())) return "—";
  const diff = Date.now() - date.getTime();
  const days = Math.floor(diff / 86_400_000);
  if (days <= 0) return "today";
  if (days === 1) return "yesterday";
  if (days < 30) return `${days}d ago`;
  const months = Math.floor(days / 30);
  if (months < 12) return `${months}mo ago`;
  return `${Math.floor(months / 12)}y ago`;
}

export function initials(value: string): string {
  const cleaned = value.replace(/[^a-zA-Z0-9 ]/g, " ").trim();
  if (!cleaned) return "JS";
  const words = cleaned.split(/\s+/);
  if (words.length === 1) return words[0].slice(0, 2).toUpperCase();
  return `${words[0][0]}${words[1][0]}`.toUpperCase();
}

const GRADIENTS = [
  "from-acid-400/25 via-emerald-400/10 to-transparent",
  "from-sky-400/25 via-cyan-400/10 to-transparent",
  "from-violet-400/25 via-fuchsia-400/10 to-transparent",
  "from-amber-400/25 via-orange-400/10 to-transparent",
  "from-rose-400/25 via-pink-400/10 to-transparent",
  "from-teal-400/25 via-emerald-400/10 to-transparent",
];

export function gradientFor(seed: string): string {
  let hash = 0;
  for (let index = 0; index < seed.length; index += 1) {
    hash = (hash * 31 + seed.charCodeAt(index)) >>> 0;
  }
  return GRADIENTS[hash % GRADIENTS.length];
}

export function basename(path: string): string {
  const normalized = path.replace(/\\/g, "/");
  return normalized.slice(normalized.lastIndexOf("/") + 1);
}

export function stripExtension(name: string): string {
  const index = name.lastIndexOf(".");
  return index > 0 ? name.slice(0, index) : name;
}

export function uid(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) {
    return crypto.randomUUID();
  }
  return `req-${Date.now()}-${Math.random().toString(16).slice(2)}`;
}

export function errorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return "Something went wrong. Check the engine log and try again.";
}

const ARCHITECTURES: Array<[string, string]> = [
  ["qwen3.8", "Qwen3.8"],
  ["qwen3.6", "Qwen3.6"],
  ["qwen3.5", "Qwen3.5"],
  ["qwen3", "Qwen3"],
  ["qwen2", "Qwen2"],
  ["llama", "Llama"],
  ["gemma", "Gemma"],
  ["mixtral", "Mixtral"],
  ["mistral", "Mistral"],
  ["deepseek", "DeepSeek"],
  ["falcon", "Falcon"],
  ["mamba", "Mamba"],
  ["rwkv", "RWKV"],
  ["olmo", "OLMo"],
  ["phi", "Phi"],
  ["glm", "GLM"],
];

/// The Hub's pipeline tag describes the task, not the architecture. Prefer the
/// model family found in its id or tags, and fall back to the task label.
export function architectureFor(model: HfModel): string {
  const haystack = `${model.id} ${model.tags.join(" ")}`.toLowerCase();
  const match = ARCHITECTURES.find(([needle]) => haystack.includes(needle));
  if (match) return match[1];
  if (model.pipeline_tag) return model.pipeline_tag.replace(/-/g, " ");
  return "Unknown";
}

export function tokensPerSecond(
  completionTokens: number | undefined,
  elapsedMs: number | null,
): string {
  if (!completionTokens || !elapsedMs || elapsedMs <= 0) return "—";
  return `${(completionTokens / (elapsedMs / 1000)).toFixed(1)} tok/s`;
}
