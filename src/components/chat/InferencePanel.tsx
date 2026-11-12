import { Loader2, RotateCcw, SlidersHorizontal } from "lucide-react";

import { basename, formatDuration, tokensPerSecond } from "../../lib/format";
import type {
  ChatUsage,
  EngineStatus,
  LocalModel,
} from "../../types";
import { Slider } from "../ui/Slider";
import { StatusPill } from "../ui/StatusPill";

interface InferencePanelProps {
  models: LocalModel[];
  currentModelPath: string;
  onChooseModel: (path: string) => void;
  switchingModel: boolean;
  systemPrompt: string;
  onSystemPromptChange: (value: string) => void;
  temperature: number;
  onTemperatureChange: (value: number) => void;
  topP: number;
  onTopPChange: (value: number) => void;
  usage: ChatUsage | null;
  elapsedMs: number | null;
  engineStatus: EngineStatus;
}

export function InferencePanel({
  models,
  currentModelPath,
  onChooseModel,
  switchingModel,
  systemPrompt,
  onSystemPromptChange,
  temperature,
  onTemperatureChange,
  topP,
  onTopPChange,
  usage,
  elapsedMs,
  engineStatus,
}: InferencePanelProps) {
  return (
    <aside className="hidden w-[336px] shrink-0 flex-col border-l border-white/5 bg-ink-900/40 lg:flex">
      <div className="flex h-12 shrink-0 items-center gap-2 border-b border-white/5 px-5">
        <SlidersHorizontal className="h-3.5 w-3.5 text-slate-500" />
        <span className="text-[11px] font-semibold uppercase tracking-[0.14em] text-slate-400">
          Inference
        </span>
        <StatusPill status={engineStatus} className="ml-auto" />
      </div>

      <div className="min-h-0 flex-1 space-y-6 overflow-y-auto px-5 py-5">
        <section className="space-y-2.5">
          <div className="flex items-center justify-between">
            <label htmlFor="model-select" className="label">
              Model
            </label>
            {switchingModel ? (
              <span className="flex items-center gap-1 text-[10px] text-slate-500">
                <Loader2 className="h-3 w-3 animate-spin" />
                restarting
              </span>
            ) : null}
          </div>
          <select
            id="model-select"
            className="field text-xs"
            value={currentModelPath}
            disabled={switchingModel}
            onChange={(event) => onChooseModel(event.target.value)}
          >
            <option value="">Select a .jns model…</option>
            {models.map((model) => (
              <option key={model.path} value={model.path}>
                {model.name}
              </option>
            ))}
            {currentModelPath &&
            !models.some((model) => model.path === currentModelPath) ? (
              <option value={currentModelPath}>
                {basename(currentModelPath)} (custom)
              </option>
            ) : null}
          </select>
          <p className="text-[10px] leading-4 text-slate-600">
            Changing the model saves it to Settings and restarts a running
            engine.
          </p>
        </section>

        <div className="h-px bg-white/5" />

        <section className="space-y-2.5">
          <div className="flex items-center justify-between">
            <label htmlFor="system-prompt" className="label">
              System prompt
            </label>
            <span className="text-[10px] text-slate-600">
              {systemPrompt.length} chars
            </span>
          </div>
          <textarea
            id="system-prompt"
            rows={6}
            value={systemPrompt}
            onChange={(event) => onSystemPromptChange(event.target.value)}
            placeholder="Set the model's global context and rules. This is sent as the system message before every conversation."
            className="field resize-none text-[11.5px] leading-5"
          />
          <div className="flex items-center justify-between">
            <span className="text-[10px] text-slate-600">
              Applied to every request
            </span>
            <button
              type="button"
              onClick={() => onSystemPromptChange("")}
              disabled={!systemPrompt}
              className="inline-flex items-center gap-1 text-[10px] text-slate-500 transition hover:text-slate-300 disabled:opacity-40"
            >
              <RotateCcw className="h-2.5 w-2.5" />
              Reset
            </button>
          </div>
        </section>

        <div className="h-px bg-white/5" />

        <section className="space-y-5">
          <div className="flex items-center justify-between">
            <span className="label">Sampling</span>
            <span className="rounded-full border border-white/10 bg-white/5 px-2 py-0.5 text-[9px] uppercase tracking-wider text-slate-500">
              request payload
            </span>
          </div>

          <Slider
            label="Temperature"
            value={temperature}
            min={0}
            max={2}
            step={0.05}
            onChange={onTemperatureChange}
            leftHint="0.0 · precise"
            rightHint="2.0 · wild"
            description="Scales the next-token distribution. Zero is greedy; higher values allow more variety."
          />

          <Slider
            label="Top-P"
            value={topP}
            min={0}
            max={1}
            step={0.01}
            onChange={onTopPChange}
            leftHint="0.0 · narrow"
            rightHint="1.0 · full"
            description="Keeps the smallest set of tokens whose cumulative probability reaches this value."
          />
        </section>

        <div className="h-px bg-white/5" />

        <section className="space-y-3">
          <span className="label">This session</span>
          <div className="grid grid-cols-2 gap-2">
            <Stat label="Prompt tokens" value={tokenLabel(usage?.prompt_tokens)} />
            <Stat
              label="Reply tokens"
              value={tokenLabel(usage?.completion_tokens)}
            />
            <Stat
              label="Speed"
              value={tokensPerSecond(usage?.completion_tokens, elapsedMs)}
            />
            <Stat
              label="Elapsed"
              value={elapsedMs ? formatDuration(elapsedMs) : "—"}
            />
          </div>
        </section>

        {engineStatus !== "ready" ? (
          <p className="rounded-xl border border-amber-400/15 bg-amber-400/5 px-3 py-2.5 text-[10px] leading-4 text-amber-200/80">
            The engine is not ready. Start janas-server from the sidebar before
            sending a message.
          </p>
        ) : null}
      </div>
    </aside>
  );
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-xl border border-white/5 bg-white/[0.02] px-3 py-2">
      <p className="text-[9px] uppercase tracking-wider text-slate-600">
        {label}
      </p>
      <p className="mt-1 truncate font-mono text-[11px] text-slate-300">
        {value}
      </p>
    </div>
  );
}

function tokenLabel(value: number | undefined): string {
  if (value === undefined || value === null) return "—";
  return new Intl.NumberFormat("en").format(value);
}
