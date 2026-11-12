import { useEffect, useState } from "react";
import { ArrowDownToLine, ArrowUpFromLine, Gauge } from "lucide-react";

import { cn } from "../../lib/cn";
import type { ChatUsage } from "../../types";

interface TokenTickerProps {
  usage: ChatUsage | null;
  elapsedMs: number | null;
  firstTokenMs: number | null;
  liveBytes: number;
  streaming: boolean;
  startedAtMs: number | null;
}

/**
 * Prompt ingestion and reply generation are timed separately: the gap before
 * the first delta is disk/prefill latency, everything after it is sequential
 * generation.
 */
export function TokenTicker({
  usage,
  elapsedMs,
  firstTokenMs,
  liveBytes,
  streaming,
  startedAtMs,
}: TokenTickerProps) {
  const [now, setNow] = useState(Date.now());

  useEffect(() => {
    if (!streaming) return;
    const timer = window.setInterval(() => setNow(Date.now()), 400);
    return () => window.clearInterval(timer);
  }, [streaming]);

  const promptTokens = usage?.prompt_tokens ?? 0;
  const completionTokens = usage?.completion_tokens ?? 0;
  const prefillSeconds = (firstTokenMs ?? 0) / 1000;
  const promptSpeed =
    promptTokens > 0 && prefillSeconds > 0.01
      ? promptTokens / prefillSeconds
      : null;
  const generationMs = Math.max(0, (elapsedMs ?? 0) - (firstTokenMs ?? 0));
  const replySpeed =
    completionTokens > 0 && generationMs > 0
      ? completionTokens / (generationMs / 1000)
      : null;

  const liveSeconds =
    streaming && startedAtMs ? Math.max(0.4, (now - startedAtMs) / 1000) : 0;
  const liveTokens = liveBytes / 4;
  const liveSpeed = liveSeconds ? liveTokens / liveSeconds : 0;

  return (
    <div className="rounded-xl border border-white/5 bg-ink-950/50 p-3">
      <div className="mb-2 flex items-center gap-1.5 text-[9px] uppercase tracking-[0.16em] text-slate-600">
        <Gauge className="h-3 w-3" />
        token generation
        {streaming ? (
          <span className="ml-auto inline-flex items-center gap-1 text-acid-300/80">
            <span className="h-1 w-1 animate-pulse-soft rounded-full bg-acid-400" />
            live
          </span>
        ) : null}
      </div>
      <div className="grid grid-cols-2 gap-2">
        <Ticker
          icon={<ArrowDownToLine className="h-3 w-3" />}
          label="Prompt read"
          value={promptSpeed ? `${promptSpeed.toFixed(1)}` : "—"}
          unit="tok/s"
          detail={
            firstTokenMs
              ? `${firstTokenMs} ms to first token`
              : "disk / prefill latency"
          }
          tone="sky"
        />
        <Ticker
          icon={<ArrowUpFromLine className="h-3 w-3" />}
          label="Reply write"
          value={
            streaming
              ? liveSpeed.toFixed(1)
              : replySpeed
                ? replySpeed.toFixed(1)
                : "—"
          }
          unit="tok/s"
          detail={
            streaming
              ? `${Math.round(liveTokens)} tokens so far`
              : completionTokens
                ? `${completionTokens} tokens generated`
                : "sequential decode"
          }
          tone="acid"
        />
      </div>
      <div className="mt-2 flex items-center justify-between font-mono text-[9px] text-slate-600">
        <span>prompt {promptTokens || "—"}</span>
        <span>reply {completionTokens || "—"}</span>
        <span>
          {elapsedMs ? `${(elapsedMs / 1000).toFixed(1)} s wall` : "—"}
        </span>
      </div>
    </div>
  );
}

function Ticker({
  icon,
  label,
  value,
  unit,
  detail,
  tone,
}: {
  icon: React.ReactNode;
  label: string;
  value: string;
  unit: string;
  detail: string;
  tone: "acid" | "sky";
}) {
  return (
    <div className="rounded-lg border border-white/5 bg-white/[0.02] px-2.5 py-2">
      <div className="flex items-center gap-1.5 text-[9px] uppercase tracking-wider text-slate-600">
        <span className={tone === "acid" ? "text-acid-400/80" : "text-sky-400/80"}>
          {icon}
        </span>
        {label}
      </div>
      <div className="mt-1 flex items-baseline gap-1">
        <span
          className={cn(
            "font-mono text-lg font-semibold tabular-nums",
            tone === "acid" ? "text-acid-200" : "text-sky-200",
          )}
        >
          {value}
        </span>
        <span className="text-[9px] text-slate-600">{unit}</span>
      </div>
      <p className="mt-0.5 truncate text-[9px] text-slate-600">{detail}</p>
    </div>
  );
}
