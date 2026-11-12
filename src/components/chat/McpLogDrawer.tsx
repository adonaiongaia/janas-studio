import { useEffect, useState } from "react";
import { Terminal, Trash2, X } from "lucide-react";

import { cn } from "../../lib/cn";
import {
  clearMcpLog,
  readMcpLog,
  subscribeMcpLog,
  type McpLogEntry,
} from "../../lib/mcpLog";

interface McpLogDrawerProps {
  open: boolean;
  onClose: () => void;
}

export function McpLogDrawer({ open, onClose }: McpLogDrawerProps) {
  const [log, setLog] = useState<McpLogEntry[]>(readMcpLog);

  useEffect(() => {
    setLog(readMcpLog());
    return subscribeMcpLog(() => setLog(readMcpLog()));
  }, []);

  if (!open) return null;

  return (
    <div className="fixed bottom-16 right-4 z-40 w-[380px] overflow-hidden rounded-2xl border border-white/10 bg-ink-900 shadow-panel animate-fade-in">
      <div className="flex items-center gap-2 border-b border-white/5 px-3.5 py-2.5">
        <Terminal className="h-3.5 w-3.5 text-acid-400/80" />
        <span className="text-[10px] font-semibold uppercase tracking-[0.16em] text-slate-300">
          MCP tool activity
        </span>
        <span className="text-[9px] text-slate-600">{log.length}</span>
        <div className="ml-auto flex items-center gap-1">
          <button
            type="button"
            onClick={() => {
              clearMcpLog();
              setLog([]);
            }}
            className="rounded p-1 text-slate-600 transition hover:text-slate-300"
            aria-label="Clear MCP log"
          >
            <Trash2 className="h-3 w-3" />
          </button>
          <button
            type="button"
            onClick={onClose}
            className="rounded p-1 text-slate-600 transition hover:text-slate-200"
            aria-label="Close MCP log"
          >
            <X className="h-3 w-3" />
          </button>
        </div>
      </div>
      <div className="max-h-[300px] overflow-y-auto p-2">
        {log.length ? (
          log.map((entry) => (
            <div
              key={entry.id}
              className="rounded-lg px-2 py-1.5 hover:bg-white/[0.02]"
            >
              <div className="flex items-center gap-1.5 text-[9px] text-slate-600">
                <span className="font-mono">
                  {new Date(entry.at).toLocaleTimeString([], {
                    hour: "2-digit",
                    minute: "2-digit",
                    second: "2-digit",
                  })}
                </span>
                <span className="text-slate-500">{entry.server}</span>
                <span className="uppercase tracking-wider text-slate-700">
                  {entry.kind}
                </span>
              </div>
              <p
                className={cn(
                  "mt-0.5 break-words font-mono text-[9.5px] leading-4",
                  entry.kind === "error" ? "text-rose-300/80" : "text-slate-400",
                )}
              >
                {entry.message}
              </p>
            </div>
          ))
        ) : (
          <p className="px-2 py-4 text-center text-[10px] leading-5 text-slate-600">
            No MCP activity yet. Register and probe a server in the MCP Hub; its
            handshake and tool calls are logged here, inside the chat.
          </p>
        )}
      </div>
    </div>
  );
}
