import { useEffect, useState } from "react";

import { api } from "../../lib/api";

const ALL_LAYERS = 999;

function useGgufLayerCount(modelPath: string) {
  const [layerCount, setLayerCount] = useState<number | null>(null);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    let cancelled = false;
    const trimmed = modelPath.trim();
    setLayerCount(null);

    if (!trimmed.toLowerCase().endsWith(".gguf")) {
      setLoading(false);
      return;
    }

    setLoading(true);
    const timer = window.setTimeout(() => {
      void api
        .inspectGgufModel(trimmed)
        .then((info) => {
          if (cancelled) return;
          setLayerCount(
            info.layer_count !== null && info.layer_count > 0
              ? info.layer_count
              : null,
          );
        })
        .catch(() => {
          if (!cancelled) setLayerCount(null);
        })
        .finally(() => {
          if (!cancelled) setLoading(false);
        });
    }, 250);

    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [modelPath]);

  return { layerCount, loading };
}

interface GpuOffloadControlProps {
  modelPath: string;
  value: number;
  onChange: (value: number) => void;
  disabled?: boolean;
}

/**
 * `--n-gpu-layers` control bounded by the selected GGUF's real block count.
 * llama.cpp uses 999 as an “all layers” sentinel, so that remains the fallback
 * while metadata is unavailable.
 */
export function GpuOffloadControl({
  modelPath,
  value,
  onChange,
  disabled = false,
}: GpuOffloadControlProps) {
  const { layerCount, loading } = useGgufLayerCount(modelPath);
  const limit = layerCount ?? ALL_LAYERS;
  const displayValue = Math.min(limit, Math.max(0, Math.round(value)));

  useEffect(() => {
    if (value !== displayValue) onChange(displayValue);
  }, [displayValue, onChange, value]);

  const setValue = (next: number) => {
    const rounded = Number.isFinite(next) ? Math.round(next) : 0;
    onChange(Math.min(limit, Math.max(0, rounded)));
  };

  const hint = loading
    ? "Reading GGUF layer count…"
    : layerCount !== null
      ? `${layerCount} layers detected in this model. 0 keeps all layers on the CPU.`
      : modelPath.trim().toLowerCase().endsWith(".gguf")
        ? "Could not read this file's layer count. 999 asks llama.cpp for every layer."
        : "Select a GGUF model to read its layer count. 999 asks llama.cpp for every layer.";

  return (
    <div className="space-y-2">
      <div className="flex items-center gap-3">
        <input
          type="range"
          className="range-acid min-w-0 flex-1"
          min={0}
          max={limit}
          step={1}
          value={displayValue}
          disabled={disabled}
          onChange={(event) => setValue(Number(event.target.value))}
          aria-label="GPU layers to offload"
          aria-valuetext={`${displayValue} of ${limit} layers`}
        />
        <div className="flex shrink-0 items-center gap-1.5">
          <input
            type="number"
            min={0}
            max={limit}
            step={1}
            value={displayValue}
            disabled={disabled}
            onChange={(event) => setValue(Number(event.target.value))}
            className="field !w-16 !px-2 !py-1.5 text-right font-mono text-[11px]"
            aria-label="GPU layers to offload value"
          />
          <span className="font-mono text-[9px] text-slate-600">/ {limit}</span>
        </div>
      </div>
      <p className="text-[9.5px] leading-4 text-slate-600">{hint}</p>
    </div>
  );
}