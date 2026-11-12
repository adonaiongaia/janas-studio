import { useState } from "react";
import {
  Brain,
  Check,
  ChevronLeft,
  ChevronRight,
  MessageSquare,
  Pencil,
  Plus,
  Trash2,
  X,
} from "lucide-react";

import { cn } from "../../lib/cn";
import type { MemoryController } from "../../lib/memory";
import type { ChatSession } from "../../types";

interface SessionsRailProps {
  sessions: ChatSession[];
  activeId: string;
  streamingSessionId: string | null;
  collapsed: boolean;
  onToggle: () => void;
  onSelect: (id: string) => void;
  onNew: () => void;
  onDelete: (id: string) => void;
  memory: MemoryController;
}

type Tab = "chats" | "memory";

export function SessionsRail({
  sessions,
  activeId,
  streamingSessionId,
  collapsed,
  onToggle,
  onSelect,
  onNew,
  onDelete,
  memory,
}: SessionsRailProps) {
  const [tab, setTab] = useState<Tab>("chats");

  if (collapsed) {
    return (
      <aside className="flex w-10 shrink-0 flex-col items-center gap-2 border-r border-white/5 bg-ink-900/40 py-3">
        <button
          type="button"
          onClick={onToggle}
          className="rounded-lg p-1.5 text-slate-500 transition hover:bg-white/5 hover:text-slate-200"
          aria-label="Expand chats"
        >
          <ChevronRight className="h-4 w-4" />
        </button>
        <button
          type="button"
          onClick={onNew}
          className="rounded-lg p-1.5 text-slate-500 transition hover:bg-white/5 hover:text-acid-300"
          aria-label="New chat"
        >
          <Plus className="h-4 w-4" />
        </button>
        <span className="mt-1 h-px w-5 bg-white/10" />
        <span className="text-[9px] font-semibold uppercase tracking-wider text-slate-600">
          {sessions.length}
        </span>
      </aside>
    );
  }

  return (
    <aside className="flex w-[226px] shrink-0 flex-col border-r border-white/5 bg-ink-900/40">
      <div className="flex h-12 shrink-0 items-center gap-1 border-b border-white/5 px-3">
        <div className="flex flex-1 rounded-lg border border-white/5 bg-ink-950/60 p-0.5">
          <button
            type="button"
            onClick={() => setTab("chats")}
            className={cn(
              "flex flex-1 items-center justify-center gap-1.5 rounded-md px-2 py-1 text-[10px] font-medium transition",
              tab === "chats"
                ? "bg-white/[0.06] text-slate-200"
                : "text-slate-500 hover:text-slate-300",
            )}
          >
            <MessageSquare className="h-3 w-3" />
            Chats
          </button>
          <button
            type="button"
            onClick={() => setTab("memory")}
            className={cn(
              "flex flex-1 items-center justify-center gap-1.5 rounded-md px-2 py-1 text-[10px] font-medium transition",
              tab === "memory"
                ? "bg-white/[0.06] text-slate-200"
                : "text-slate-500 hover:text-slate-300",
            )}
          >
            <Brain className="h-3 w-3" />
            Memory
          </button>
        </div>
        <button
          type="button"
          onClick={onToggle}
          className="rounded-lg p-1 text-slate-500 transition hover:bg-white/5 hover:text-slate-200"
          aria-label="Collapse chats"
        >
          <ChevronLeft className="h-3.5 w-3.5" />
        </button>
      </div>

      {tab === "chats" ? (
        <>
          <div className="px-2.5 py-2.5">
            <button
              type="button"
              onClick={onNew}
              className="flex w-full items-center gap-2 rounded-lg border border-dashed border-white/10 px-2.5 py-2 text-[11px] text-slate-400 transition hover:border-acid-400/30 hover:text-acid-200"
            >
              <Plus className="h-3.5 w-3.5" />
              New chat
            </button>
          </div>
          <div className="min-h-0 flex-1 space-y-0.5 overflow-y-auto px-2 pb-3">
            {sessions.map((session) => {
              const active = session.id === activeId;
              return (
                <div
                  key={session.id}
                  className={cn(
                    "group flex items-start gap-2 rounded-lg px-2 py-2 transition",
                    active
                      ? "bg-white/[0.06]"
                      : "hover:bg-white/[0.03]",
                  )}
                >
                  <button
                    type="button"
                    onClick={() => onSelect(session.id)}
                    className="min-w-0 flex-1 text-left"
                  >
                    <span
                      className={cn(
                        "block truncate text-[11.5px]",
                        active ? "text-slate-100" : "text-slate-400",
                      )}
                    >
                      {session.title || "New chat"}
                    </span>
                    <span className="mt-0.5 flex items-center gap-1.5 text-[9px] text-slate-600">
                      {session.messages.length
                        ? `${Math.ceil(session.messages.length / 2)} turns`
                        : "empty"}
                      {streamingSessionId === session.id ? (
                        <span className="inline-flex items-center gap-1 text-acid-300">
                          <span className="h-1 w-1 animate-pulse-soft rounded-full bg-acid-400" />
                          live
                        </span>
                      ) : null}
                    </span>
                  </button>
                  <button
                    type="button"
                    onClick={() => onDelete(session.id)}
                    className="rounded p-0.5 text-slate-600 opacity-0 transition hover:text-rose-300 group-hover:opacity-100"
                    aria-label="Delete chat"
                  >
                    <Trash2 className="h-3 w-3" />
                  </button>
                </div>
              );
            })}
          </div>
        </>
      ) : (
        <MemoryPanel memory={memory} />
      )}
    </aside>
  );
}

function MemoryPanel({ memory }: { memory: MemoryController }) {
  const [draft, setDraft] = useState("");
  const [editing, setEditing] = useState<string | null>(null);
  const [editText, setEditText] = useState("");

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="px-3 py-2.5">
        <p className="text-[10px] leading-4 text-slate-500">
          The model saves durable facts here automatically. Entries are recalled
          by local vector similarity and sent as context when relevant.
        </p>
        {memory.loading ? (
          <p className="mt-1 text-[10px] leading-4 text-slate-600">
            Loading the memory bank…
          </p>
        ) : null}
        {memory.error ? (
          <p className="mt-1 text-[10px] leading-4 text-rose-300">
            {memory.error}
          </p>
        ) : null}
        <div className="mt-2 flex gap-1.5">
          <input
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter" && draft.trim()) {
                void memory.add(draft);
                setDraft("");
              }
            }}
            placeholder="Add a fact…"
            className="field !px-2.5 !py-1.5 text-[11px]"
          />
          <button
            type="button"
            onClick={() => {
              if (!draft.trim()) return;
              void memory.add(draft);
              setDraft("");
            }}
            className="rounded-lg border border-white/10 bg-white/5 px-2 text-slate-300 transition hover:bg-white/10"
            aria-label="Add memory"
          >
            <Plus className="h-3.5 w-3.5" />
          </button>
        </div>
      </div>

      <div className="min-h-0 flex-1 space-y-1 overflow-y-auto px-2 pb-3">
        {!memory.facts.length && !memory.loading ? (
          <p className="px-1 py-6 text-center text-[10px] leading-5 text-slate-600">
            Nothing memorised yet. Ask the model to remember something, or add a
            fact here. Delete stale entries so it never loops on an old
            configuration.
          </p>
        ) : null}

        {memory.facts.map((fact) => (
          <div
            key={fact.id}
            className="group rounded-lg border border-white/5 bg-white/[0.02] px-2 py-1.5"
          >
            {editing === fact.id ? (
              <div className="flex items-start gap-1">
                <textarea
                  value={editText}
                  onChange={(event) => setEditText(event.target.value)}
                  rows={3}
                  className="field min-w-0 flex-1 resize-none !px-2 !py-1 text-[10.5px] leading-4"
                />
                <div className="flex flex-col gap-1">
                  <button
                    type="button"
                    onClick={() => {
                      void memory.edit(fact.id, editText);
                      setEditing(null);
                    }}
                    className="rounded p-0.5 text-acid-300"
                    aria-label="Save memory"
                  >
                    <Check className="h-3 w-3" />
                  </button>
                  <button
                    type="button"
                    onClick={() => setEditing(null)}
                    className="rounded p-0.5 text-slate-500"
                    aria-label="Cancel edit"
                  >
                    <X className="h-3 w-3" />
                  </button>
                </div>
              </div>
            ) : (
              <div className="flex items-start gap-1.5">
                <p className="min-w-0 flex-1 whitespace-pre-wrap text-[10.5px] leading-4 text-slate-400">
                  {fact.text}
                </p>
                <div className="flex shrink-0 flex-col gap-1 opacity-0 transition group-hover:opacity-100">
                  <button
                    type="button"
                    onClick={() => {
                      setEditing(fact.id);
                      setEditText(fact.text);
                    }}
                    className="rounded p-0.5 text-slate-600 hover:text-slate-300"
                    aria-label="Edit memory"
                  >
                    <Pencil className="h-2.5 w-2.5" />
                  </button>
                  <button
                    type="button"
                    onClick={() => void memory.remove(fact.id)}
                    className="rounded p-0.5 text-slate-600 hover:text-rose-300"
                    aria-label="Forget memory"
                  >
                    <Trash2 className="h-2.5 w-2.5" />
                  </button>
                </div>
              </div>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}
