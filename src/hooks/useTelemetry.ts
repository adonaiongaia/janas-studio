import { useCallback, useEffect, useRef, useState } from "react";

import { api } from "../lib/api";
import type { Telemetry } from "../types";

export interface TelemetryPoint {
  at: number;
  read_mb_s: number;
  iops: number;
  cpu: number;
  rss_bytes: number;
}

const EMPTY: Telemetry = {
  pid: null,
  process_rss_bytes: 0,
  process_cpu_percent: 0,
  read_bytes_per_sec: 0,
  read_ops_per_sec: 0,
  model_bytes: 0,
  model_name: "—",
  resident_ratio: 0,
  cores: 1,
  mem_total_bytes: 0,
  mem_available_bytes: 0,
  gpu: {
    present: false,
    name: "",
    utilization: null,
    memory_used: null,
    memory_total: null,
  },
  npu: {
    present: false,
    name: "",
    utilization: null,
    memory_used: null,
    memory_total: null,
  },
  sampled_at_ms: 0,
};

export function useTelemetry(pid: number | null, enabled: boolean) {
  const [telemetry, setTelemetry] = useState<Telemetry>(EMPTY);
  const [history, setHistory] = useState<TelemetryPoint[]>([]);
  const [error, setError] = useState<string | null>(null);
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const sample = useCallback(async () => {
    try {
      const next = await api.getTelemetry(pid);
      if (!mounted.current) return;
      setTelemetry(next);
      setError(null);
      setHistory((previous) => {
        const point: TelemetryPoint = {
          at: next.sampled_at_ms,
          read_mb_s: next.read_bytes_per_sec / (1024 * 1024),
          iops: next.read_ops_per_sec,
          cpu: next.process_cpu_percent,
          rss_bytes: next.process_rss_bytes,
        };
        return [...previous, point].slice(-90);
      });
    } catch (err) {
      if (mounted.current) {
        setError(err instanceof Error ? err.message : String(err));
      }
    }
  }, [pid]);

  useEffect(() => {
    if (!enabled) return;
    void sample();
    const timer = window.setInterval(() => void sample(), 1_200);
    return () => window.clearInterval(timer);
  }, [enabled, sample]);

  return { telemetry, history, error, sample };
}

export type TelemetryController = ReturnType<typeof useTelemetry>;
