import {
  Binary,
  Boxes,
  Compass,
  Cpu,
  Loader2,
  MessageSquare,
  Play,
  Plug,
  Settings,
  Square,
} from "lucide-react";

import { cn } from "../lib/cn";
import { engineLabel } from "../lib/engine-controller";
import { basename } from "../lib/format";
import type { EngineController } from "../hooks/useEngine";
import type { ViewId } from "../types";
import { Logo } from "./Logo";
import { Button } from "./ui/Button";
import { StatusPill } from "./ui/StatusPill";

interface SidebarProps {
  view: ViewId;
  onNavigate: (view: ViewId) => void;
  engine: EngineController;
}

const NAV: Array<{
  id: ViewId;
  label: string;
  hint: string;
  icon: typeof MessageSquare;
}> = [
  { id: "chat", label: "Chat", hint: "Streaming workspace", icon: MessageSquare },
  { id: "models", label: "Models", hint: "Hugging Face explorer", icon: Compass },
  { id: "library", label: "Library", hint: "Local model shelf", icon: Boxes },
  { id: "compiler", label: "Compiler", hint: "GGUF to JNS", icon: Binary },
  { id: "mcp", label: "MCP Hub", hint: "Local tool servers", icon: Plug },
  { id: "settings", label: "Settings", hint: "Runtime and paths", icon: Settings },
];

export function Sidebar({ view, onNavigate, engine }: SidebarProps) {
  const { snapshot, busy } = engine;
  const running = snapshot.status !== "disconnected";

  return (
    <aside className="flex w-[248px] shrink-0 flex-col border-r border-white/5 bg-ink-900/50">
      <div className="flex items-center gap-3 px-5 py-5">
        <Logo className="h-9 w-9" />
        <div className="min-w-0">
          <p className="text-sm font-semibold tracking-tight text-white">
            Janas Studio
          </p>
          <p className="text-[10px] uppercase tracking-[0.18em] text-slate-600">
            local inference
          </p>
        </div>
      </div>

      <nav className="space-y-1 px-3">
        {NAV.map((item) => {
          const Icon = item.icon;
          const active = view === item.id;
          return (
            <button
              key={item.id}
              type="button"
              onClick={() => onNavigate(item.id)}
              className={cn(
                "group flex w-full items-center gap-3 rounded-xl px-3 py-2.5 text-left transition",
                active
                  ? "bg-white/[0.06] text-white"
                  : "text-slate-400 hover:bg-white/[0.03] hover:text-slate-200",
              )}
            >
              <span
                className={cn(
                  "flex h-7 w-7 items-center justify-center rounded-lg border transition",
                  active
                    ? "border-acid-400/30 bg-acid-400/10 text-acid-300"
                    : "border-white/5 bg-white/[0.02] text-slate-500 group-hover:text-slate-300",
                )}
              >
                <Icon className="h-3.5 w-3.5" />
              </span>
              <span className="min-w-0 flex-1">
                <span className="block text-[13px] font-medium">
                  {item.label}
                </span>
                <span className="block truncate text-[10px] text-slate-600">
                  {item.hint}
                </span>
              </span>
            </button>
          );
        })}
      </nav>

      <div className="mt-auto space-y-3 p-3">
        <div className="rounded-2xl border border-white/10 bg-ink-850 p-3.5">
          <div className="flex items-center justify-between">
            <StatusPill
              status={snapshot.status}
              detail={snapshot.detail}
            />
            <span className="flex items-center gap-1 text-[10px] text-slate-600">
              <Cpu className="h-3 w-3" />
              :{snapshot.port}
            </span>
          </div>

          <p className="mt-3 flex items-center gap-1.5 text-[9px] uppercase tracking-[0.16em] text-slate-600">
            <Cpu className="h-2.5 w-2.5" />
            {engineLabel(engine.active?.kind, "engine idle")}
          </p>
          <p className="mt-1.5 truncate text-xs font-medium text-slate-300">
            {snapshot.model ? basename(snapshot.model) : "No model selected"}
          </p>
          <p className="mt-0.5 line-clamp-2 text-[10px] leading-4 text-slate-600">
            {snapshot.detail}
          </p>

          {engine.error ? (
            <p className="mt-2 text-[10px] leading-4 text-rose-300">
              {engine.error}
            </p>
          ) : null}
          {engine.adapterWarning ? (
            <p className="mt-2 text-[10px] leading-4 text-amber-300/80">
              {engine.adapterWarning.message}
            </p>
          ) : null}

          <Button
            size="sm"
            variant={running ? "secondary" : "primary"}
            busy={busy}
            className="mt-3 w-full justify-center"
            onClick={() => {
              if (running) void engine.stop();
              else void engine.start();
            }}
            icon={
              busy ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
              ) : running ? (
                <Square className="h-3 w-3" />
              ) : (
                <Play className="h-3 w-3" />
              )
            }
          >
            {running ? "Stop engine" : "Start engine"}
          </Button>
        </div>

        <p className="px-1 text-[10px] leading-4 text-slate-600">
          {engine.active?.kind === "llama"
            ? "llama.cpp runs on this machine. Prompts never leave it."
            : "janas-server runs on this machine. Prompts never leave it."}
        </p>
      </div>
    </aside>
  );
}
