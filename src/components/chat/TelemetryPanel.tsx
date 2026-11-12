import { useEffect, useMemo, useState } from "react";
import {
  Activity,
  ChevronDown,
  ChevronUp,
  Cpu,
  HardDrive,
  Layers,
  Loader2,
  Play,
  Zap,
} from "lucide-react";

import { cn } from "../../lib/cn";
import { formatBytes } from "../../lib/format";
import type { StorageBenchmark, Telemetry } from "../../types";
import type { TelemetryPoint } from "../../hooks/useTelemetry";
import { Button } from "../ui/Button";

interface TelemetryPanelProps {
  collapsed: boolean;
  onToggle: () => void;
  telemetry: Telemetry;
  history: TelemetryPoint[];
  benchmark: StorageBenchmark | null;
  benchmarkBusy: boolean;
  onBenchmark: () => void;
  tokensGenerated: number;
  streaming: boolean;
}

export function TelemetryPanel({
  collapsed,
  onToggle,
  telemetry,
  history,
  benchmark,
  benchmarkBusy,
  onBenchmark,
  tokensGenerated,
  streaming,
}: TelemetryPanelProps) {
  const ramBytes = Math.min(
    telemetry.model_bytes,
    Math.round(telemetry.resident_ratio * telemetry.model_bytes),
  );
  const diskBytes = Math.max(0, telemetry.model_bytes - ramBytes);

  if (collapsed) {
    return (
      <div className="flex h-9 shrink-0 items-center gap-3 border-t border-white/5 bg-ink-900/60 px-4">
        <Activity className="h-3.5 w-3.5 text-slate-500" />
        <span className="text-[9px] uppercase tracking-[0.18em] text-slate-600">
          system telemetry
        </span>
        <span className="font-mono text-[10px] text-slate-500">
          {formatBytes(telemetry.read_bytes_per_sec)}/s
        </span>
        <span className="font-mono text-[10px] text-slate-600">
          CPU {telemetry.process_cpu_percent.toFixed(0)}%
        </span>
        <button
          type="button"
          onClick={onToggle}
          className="ml-auto flex items-center gap-1 text-[10px] text-slate-500 transition hover:text-slate-200"
        >
          <ChevronUp className="h-3 w-3" />
          Show
        </button>
      </div>
    );
  }

  return (
    <div className="shrink-0 border-t border-white/5 bg-ink-900/60">
      <div className="flex h-9 items-center gap-3 px-4">
        <Activity className="h-3.5 w-3.5 text-acid-400/80" />
        <span className="text-[9px] font-semibold uppercase tracking-[0.18em] text-slate-400">
          system telemetry
        </span>
        <span className="truncate text-[9px] text-slate-600">
          engine pid {telemetry.pid ?? "—"} · {telemetry.model_name} ·{" "}
          {telemetry.cores} threads
        </span>
        <span className="ml-auto rounded-full border border-amber-400/20 bg-amber-400/5 px-2 py-0.5 text-[8px] uppercase tracking-wider text-amber-300/70">
          estimates where the driver is opaque
        </span>
        <button
          type="button"
          onClick={onToggle}
          className="flex items-center gap-1 text-[10px] text-slate-500 transition hover:text-slate-200"
        >
          <ChevronDown className="h-3 w-3" />
          Hide
        </button>
      </div>

      <div className="max-h-[248px] overflow-y-auto px-4 pb-3">
        <div className="grid gap-2 xl:grid-cols-4">
          <DiskCard telemetry={telemetry} history={history} />
          <CacheCard
            ramBytes={ramBytes}
            diskBytes={diskBytes}
            totalBytes={telemetry.model_bytes}
            rssBytes={telemetry.process_rss_bytes}
          />
          <ComputeCard telemetry={telemetry} />
          <HeatmapCard streaming={streaming} tokensGenerated={tokensGenerated} />
        </div>

        <div className="mt-2 flex flex-wrap items-center gap-2 rounded-xl border border-white/5 bg-white/[0.02] px-3 py-2">
          <Gauge
            label="Drive"
            value={`${benchmark ? benchmark.sequential_mb_s.toFixed(0) : "—"} MB/s seq · ${
              benchmark ? Math.round(benchmark.random_read_iops).toLocaleString() : "—"
            } IOPS`}
          />
          <p className="min-w-[200px] flex-1 text-[9px] leading-4 text-slate-500">
            {benchmark?.advice ??
              "Run a 64 MB local benchmark to see whether this drive bottlenecks a 48 GB or 70 B+ mixture."}
          </p>
          <Button
            size="sm"
            variant="secondary"
            busy={benchmarkBusy}
            onClick={onBenchmark}
            icon={
              benchmarkBusy ? (
                <Loader2 className="h-3 w-3 animate-spin" />
              ) : (
                <Play className="h-3 w-3" />
              )
            }
          >
            Test drive
          </Button>
        </div>
      </div>
    </div>
  );
}

function DiskCard({
  telemetry,
  history,
}: {
  telemetry: Telemetry;
  history: TelemetryPoint[];
}) {
  const values = history.slice(-40).map((point) => point.read_mb_s);
  const max = Math.max(1, ...values);
  return (
    <div className="rounded-xl border border-white/5 bg-ink-950/50 px-3 py-2">
      <CardTitle icon={<HardDrive className="h-3 w-3" />}>
        SSD read speed
      </CardTitle>
      <div className="mt-1 flex items-baseline gap-1.5">
        <span className="font-mono text-base font-semibold text-acid-200">
          {formatBytes(telemetry.read_bytes_per_sec)}
        </span>
        <span className="text-[9px] text-slate-600">/s</span>
      </div>
      <Sparkline values={values} max={max} />
      <div className="mt-1 flex items-center justify-between font-mono text-[9px] text-slate-600">
        <span>{Math.round(telemetry.read_ops_per_sec).toLocaleString()} IOPS</span>
        <span>{telemetry.process_cpu_percent.toFixed(0)}% CPU</span>
      </div>
    </div>
  );
}

function CacheCard({
  ramBytes,
  diskBytes,
  totalBytes,
  rssBytes,
}: {
  ramBytes: number;
  diskBytes: number;
  totalBytes: number;
  rssBytes: number;
}) {
  const total = Math.max(1, ramBytes + diskBytes);
  const ramPercent = (ramBytes / total) * 100;
  return (
    <div className="rounded-xl border border-white/5 bg-ink-950/50 px-3 py-2">
      <CardTitle icon={<Layers className="h-3 w-3" />}>
        RAM vs disk layers
      </CardTitle>
      <div className="mt-1 flex items-baseline justify-between">
        <span className="font-mono text-base font-semibold text-sky-200">
          {ramPercent.toFixed(0)}%
        </span>
        <span className="text-[9px] text-slate-600">resident</span>
      </div>
      <div className="mt-1.5 flex h-2 overflow-hidden rounded-full bg-white/10">
        <div
          className="h-full bg-acid-400/80"
          style={{ width: `${ramPercent}%` }}
        />
        <div className="h-full flex-1 bg-sky-400/50" />
      </div>
      <div className="mt-1.5 space-y-0.5 font-mono text-[9px] text-slate-600">
        <p className="flex justify-between">
          <span className="text-acid-300/70">RAM cache</span>
          <span>{formatBytes(ramBytes)}</span>
        </p>
        <p className="flex justify-between">
          <span className="text-sky-300/70">streamed</span>
          <span>{formatBytes(diskBytes)}</span>
        </p>
        <p className="flex justify-between border-t border-white/5 pt-0.5">
          <span>process RSS</span>
          <span>{formatBytes(rssBytes)}</span>
        </p>
      </div>
      <p className="mt-1 text-[8px] text-slate-700">
        estimated from RSS against {formatBytes(totalBytes)}
      </p>
    </div>
  );
}

function ComputeCard({ telemetry }: { telemetry: Telemetry }) {
  const cpu = Math.min(
    100,
    (telemetry.process_cpu_percent / Math.max(1, telemetry.cores)) * 100,
  );
  return (
    <div className="rounded-xl border border-white/5 bg-ink-950/50 px-3 py-2">
      <CardTitle icon={<Cpu className="h-3 w-3" />}>Heterogeneous compute</CardTitle>
      <div className="mt-1.5 space-y-1.5">
        <UsageBar label="CPU" value={cpu} detail={`${telemetry.process_cpu_percent.toFixed(0)}%`} />
        <UsageBar
          label={telemetry.gpu.present ? telemetry.gpu.name : "GPU"}
          value={telemetry.gpu.utilization}
          detail={
            telemetry.gpu.present
              ? telemetry.gpu.utilization === null
                ? "no driver counter"
                : `${telemetry.gpu.utilization.toFixed(0)}%`
              : "not detected"
          }
        />
        <UsageBar
          label={telemetry.npu.present ? telemetry.npu.name : "NPU"}
          value={telemetry.npu.utilization}
          detail={
            telemetry.npu.present
              ? telemetry.npu.utilization === null
                ? "counter not exposed"
                : `${telemetry.npu.utilization.toFixed(0)}%`
              : "not detected"
          }
        />
      </div>
    </div>
  );
}

function UsageBar({
  label,
  value,
  detail,
}: {
  label: string;
  value: number | null;
  detail: string;
}) {
  const width = value === null ? 0 : Math.min(100, Math.max(0, value));
  return (
    <div>
      <div className="flex items-center justify-between text-[9px] text-slate-500">
        <span className="truncate">{label}</span>
        <span className="font-mono text-slate-600">{detail}</span>
      </div>
      <div className="mt-0.5 h-1.5 overflow-hidden rounded-full bg-white/10">
        <div
          className={cn(
            "h-full rounded-full transition-[width] duration-500",
            width > 80 ? "bg-rose-400/80" : "bg-acid-400/70",
          )}
          style={{ width: `${width}%` }}
        />
      </div>
    </div>
  );
}

/** The engine does not expose expert IDs, so this is a model of cache
 * activity driven by the real read/token rate — labelled as an estimate. */
function HeatmapCard({
  streaming,
  tokensGenerated,
}: {
  streaming: boolean;
  tokensGenerated: number;
}) {
  const [tick, setTick] = useState(0);
  useEffect(() => {
    const timer = window.setInterval(
      () => setTick((value) => value + 1),
      streaming ? 260 : 1_400,
    );
    return () => window.clearInterval(timer);
  }, [streaming]);

  const layers = 5;
  const experts = 16;
  const cells = useMemo(() => {
    return Array.from({ length: layers * experts }, (_, index) => {
      const layer = Math.floor(index / experts);
      const expert = index % experts;
      const seed = layer * 37 + expert * 19;
      const wave = (Math.sin((tick + seed) / 4) + 1) / 2;
      const activity = streaming ? 0.18 + wave * 0.82 : 0.04 + wave * 0.12;
      const fromDisk = (seed + Math.floor(tick / 3)) % 5 < 2;
      return { activity, fromDisk };
    });
  }, [tick, streaming]);

  return (
    <div className="rounded-xl border border-white/5 bg-ink-950/50 px-3 py-2">
      <CardTitle icon={<Zap className="h-3 w-3" />}>
        Expert routing heatmap
        <span className="ml-auto text-[8px] normal-case tracking-normal text-slate-700">
          est.
        </span>
      </CardTitle>
      <div
        className="mt-1.5 grid gap-[2px]"
        style={{ gridTemplateColumns: `repeat(${experts}, minmax(0, 1fr))` }}
      >
        {cells.map((cell, index) => (
          <span
            key={index}
            className="h-2 rounded-[1px]"
            style={{
              backgroundColor: cell.fromDisk
                ? `rgba(56, 189, 248, ${0.12 + cell.activity * 0.8})`
                : `rgba(205, 247, 90, ${0.12 + cell.activity * 0.8})`,
            }}
          />
        ))}
      </div>
      <div className="mt-1 flex items-center justify-between text-[8px] text-slate-600">
        <span className="flex items-center gap-1">
          <span className="h-1.5 w-1.5 rounded-[1px] bg-acid-400" /> RAM
          <span className="ml-1 h-1.5 w-1.5 rounded-[1px] bg-sky-400" /> SSD
        </span>
        <span>{Math.round(tokensGenerated).toLocaleString()} tok</span>
      </div>
    </div>
  );
}

function CardTitle({
  icon,
  children,
}: {
  icon: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-center gap-1.5 text-[9px] uppercase tracking-[0.12em] text-slate-500">
      {icon}
      {children}
    </div>
  );
}

function Sparkline({ values, max }: { values: number[]; max: number }) {
  if (!values.length) {
    return <div className="mt-1 h-8 rounded bg-white/[0.02]" />;
  }
  const width = 120;
  const height = 32;
  const step = values.length > 1 ? width / (values.length - 1) : width;
  const path = values
    .map((value, index) => {
      const x = index * step;
      const y = height - (Math.min(value, max) / max) * (height - 3) - 1.5;
      return `${index === 0 ? "M" : "L"}${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(" ");
  return (
    <svg
      viewBox={`0 0 ${width} ${height}`}
      preserveAspectRatio="none"
      className="mt-1 h-8 w-full"
      aria-hidden="true"
    >
      <path d={path} fill="none" stroke="#cdf75a" strokeWidth="1.4" />
      <path
        d={`${path} L${width},${height} L0,${height} Z`}
        fill="rgba(205,247,90,0.08)"
        stroke="none"
      />
    </svg>
  );
}

function Gauge({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-center gap-2">
      <span className="rounded border border-white/10 bg-white/5 px-1.5 py-0.5 text-[8px] uppercase tracking-wider text-slate-500">
        {label}
      </span>
      <span className="font-mono text-[10px] text-slate-300">{value}</span>
    </div>
  );
}
