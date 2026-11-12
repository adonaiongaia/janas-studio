import { useCallback, useEffect, useState } from "react";
import {
  CheckCircle2,
  Loader2,
  Pencil,
  Play,
  Plug,
  Plus,
  Terminal,
  Trash2,
  XCircle,
} from "lucide-react";

import { usePersistentState } from "../../hooks/usePersistentState";
import { api } from "../../lib/api";
import { cn } from "../../lib/cn";
import { errorMessage, uid } from "../../lib/format";
import {
  appendMcpLog,
  clearMcpLog,
  readMcpLog,
  subscribeMcpLog,
  type McpLogEntry,
} from "../../lib/mcpLog";
import type { McpProbe, McpServerConfig } from "../../types";
import { Button } from "../ui/Button";

function splitArgs(input: string): string[] {
  const args: string[] = [];
  let current = "";
  let quote: string | null = null;
  for (const char of input) {
    if (quote) {
      if (char === quote) quote = null;
      else current += char;
      continue;
    }
    if (char === '"' || char === "'") {
      quote = char;
      continue;
    }
    if (/\s/.test(char)) {
      if (current) {
        args.push(current);
        current = "";
      }
      continue;
    }
    current += char;
  }
  if (current) args.push(current);
  return args;
}

function envNames(input: string | undefined): string[] {
  return (input ?? "")
    .split("\n")
    .map((line) => line.trim().replace(/^export\s+/, ""))
    .filter((line) => line && !line.startsWith("#") && line.includes("="))
    .map((line) => line.slice(0, line.indexOf("=")).trim())
    .filter(Boolean);
}

export function McpView() {
  const [servers, setServers] = usePersistentState<McpServerConfig[]>(
    "janas.mcpServers.v1",
    [],
  );
  const [name, setName] = useState("");
  const [command, setCommand] = useState("");
  const [args, setArgs] = useState("");
  const [env, setEnv] = useState("");
  const [cwd, setCwd] = useState("");
  const [editingId, setEditingId] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [probing, setProbing] = useState<string | null>(null);
  const [probes, setProbes] = useState<Record<string, McpProbe>>({});
  const [error, setError] = useState<string | null>(null);
  const [log, setLog] = useState<McpLogEntry[]>(readMcpLog);

  useEffect(() => {
    setLog(readMcpLog());
    return subscribeMcpLog(() => setLog(readMcpLog()));
  }, []);

  const resetEditor = () => {
    setName("");
    setCommand("");
    setArgs("");
    setEnv("");
    setCwd("");
    setEditingId(null);
    setAdding(false);
    setError(null);
  };

  const beginEdit = (server: McpServerConfig) => {
    setName(server.name);
    setCommand(server.command);
    setArgs(server.args.join(" "));
    setEnv(server.env ?? "");
    setCwd(server.cwd ?? "");
    setEditingId(server.id);
    setAdding(true);
    setError(null);
  };

  const save = () => {
    if (!command.trim()) {
      setError("An MCP server needs a command.");
      return;
    }
    const next = {
      name: name.trim() || command.split("/").pop() || "MCP server",
      command: command.trim(),
      args: splitArgs(args),
      env: env.trim(),
      cwd: cwd.trim(),
    };
    const id = editingId ?? uid();
    setServers((previous) =>
      editingId
        ? previous.map((server) =>
            server.id === editingId ? { ...server, ...next } : server,
          )
        : [...previous, { id, ...next, enabled: true }],
    );
    appendMcpLog({
      server: next.name,
      kind: "info",
      message: `${editingId ? "Updated" : "Registered"} ${next.command} ${next.args.join(" ")}`.trim(),
    });
    setProbes((previous) => {
      const updated = { ...previous };
      delete updated[id];
      return updated;
    });
    resetEditor();
  };

  const probe = useCallback(async (server: McpServerConfig) => {
    setProbing(server.id);
    setError(null);
    try {
      const result = await api.mcpProbe({
        name: server.name,
        command: server.command,
        args: server.args,
        env: server.env ?? "",
        cwd: server.cwd ?? "",
      });
      setProbes((previous) => ({ ...previous, [server.id]: result }));
      appendMcpLog({
        server: server.name,
        kind: result.ok ? "probe" : "error",
        message: result.ok
          ? `${result.message} ${result.tools.map((tool) => tool.name).join(", ")}`
          : result.message,
      });
    } catch (err) {
      setError(errorMessage(err));
      appendMcpLog({
        server: server.name,
        kind: "error",
        message: errorMessage(err),
      });
    } finally {
      setProbing(null);
    }
  }, []);

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex shrink-0 flex-wrap items-center justify-between gap-3 border-b border-white/5 px-6 py-4">
        <div className="text-[11px] leading-5 text-slate-500">
          <span className="inline-flex items-center gap-1.5 text-slate-300">
            <Plug className="h-3.5 w-3.5 text-acid-400/80" />
            MCP server hub
          </span>
          <p className="mt-0.5 text-[10px] text-slate-600">
            Register local stdio servers. Probes allow 30 seconds for startup
            and report stderr and early exits. Enabled tools are offered to the
            model in chat.
          </p>
        </div>
        <Button
          variant="primary"
          onClick={() => (adding ? resetEditor() : setAdding(true))}
          icon={<Plus className="h-3.5 w-3.5" />}
        >
          Register server
        </Button>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto px-6 py-5">
        {adding ? (
          <div className="mb-5 rounded-xl border border-white/10 bg-ink-900/60 p-4">
            <p className="text-[10px] uppercase tracking-[0.16em] text-slate-500">
              {editingId ? "Edit stdio server" : "New stdio server"}
            </p>
            <div className="mt-3 grid gap-2 md:grid-cols-[1fr_1fr_1.2fr]">
              <input
                value={name}
                onChange={(event) => setName(event.target.value)}
                placeholder="Display name"
                className="field !py-2 text-xs"
              />
              <input
                value={command}
                onChange={(event) => setCommand(event.target.value)}
                placeholder="Command (janas-mcp, npx, python…)"
                spellCheck={false}
                className="field !py-2 font-mono text-xs"
              />
              <input
                value={args}
                onChange={(event) => setArgs(event.target.value)}
                placeholder="Arguments, e.g. /models/qwen3.jns"
                spellCheck={false}
                className="field !py-2 font-mono text-xs"
              />
            </div>
            <div className="mt-2 grid gap-2 md:grid-cols-[1fr_1.5fr]">
              <label className="block">
                <span className="mb-1 block text-[9px] uppercase tracking-[0.12em] text-slate-600">
                  Working directory (optional)
                </span>
                <input
                  value={cwd}
                  onChange={(event) => setCwd(event.target.value)}
                  placeholder="/home/you/project"
                  spellCheck={false}
                  className="field !py-2 w-full font-mono text-xs"
                />
              </label>
              <label className="block">
                <span className="mb-1 block text-[9px] uppercase tracking-[0.12em] text-slate-600">
                  Environment variables (KEY=VALUE per line)
                </span>
                <textarea
                  value={env}
                  onChange={(event) => setEnv(event.target.value)}
                  placeholder={"GITHUB_TOKEN=...\nWORKSPACE=/home/you/project"}
                  spellCheck={false}
                  rows={2}
                  className="field !py-2 w-full resize-y font-mono text-xs"
                />
              </label>
            </div>
            <div className="mt-3 flex items-center gap-2">
              <Button size="sm" variant="primary" onClick={save}>
                {editingId ? "Save" : "Add"}
              </Button>
              <Button
                size="sm"
                variant="ghost"
                onClick={resetEditor}
              >
                Cancel
              </Button>
              <p className="text-[9px] text-slate-600">
                Bundled janas-mcp needs a .jns model path in Arguments.
                Environment values stay in local storage and are never logged.
              </p>
            </div>
          </div>
        ) : null}

        {error ? (
          <div className="mb-4 rounded-xl border border-rose-400/20 bg-rose-500/5 px-3.5 py-2.5 text-[11px] text-rose-300">
            {error}
          </div>
        ) : null}

        {!servers.length ? (
          <div className="flex flex-col items-center py-16 text-center">
            <Plug className="h-7 w-7 text-slate-700" />
            <p className="mt-3 text-sm font-medium text-slate-300">
              No MCP servers registered
            </p>
            <p className="mt-1 max-w-md text-[11px] leading-5 text-slate-600">
              Add a filesystem inspector, a database connector or a Python tool
              server. Probes run locally over stdio; no remote endpoint is
              required.
            </p>
          </div>
        ) : (
          <div className="space-y-2">
            {servers.map((server) => {
              const result = probes[server.id];
              return (
                <div
                  key={server.id}
                  className="rounded-xl border border-white/5 bg-ink-900/50 px-4 py-3"
                >
                  <div className="flex flex-wrap items-center gap-2">
                    <span
                      className={cn(
                        "h-1.5 w-1.5 rounded-full",
                        !server.enabled
                          ? "bg-slate-600"
                          : result
                            ? result.ok
                              ? "bg-emerald-400"
                              : "bg-rose-400"
                            : "bg-slate-500",
                      )}
                    />
                    <p className="text-[12px] font-medium text-slate-200">
                      {server.name}
                    </p>
                    <span className="truncate font-mono text-[9.5px] text-slate-600">
                      {server.command} {server.args.join(" ")}
                    </span>
                    <div className="ml-auto flex items-center gap-1.5">
                      <button
                        type="button"
                        onClick={() => beginEdit(server)}
                        className="rounded-md p-1 text-slate-600 transition hover:text-slate-200"
                        aria-label="Edit server"
                      >
                        <Pencil className="h-3 w-3" />
                      </button>
                      <button
                        type="button"
                        onClick={() =>
                          setServers((previous) =>
                            previous.map((candidate) =>
                              candidate.id === server.id
                                ? { ...candidate, enabled: !candidate.enabled }
                                : candidate,
                            ),
                          )
                        }
                        className={cn(
                          "inline-flex h-6 items-center gap-1 rounded-md border px-1.5 text-[9px] transition",
                          server.enabled
                            ? "border-acid-400/30 bg-acid-400/10 text-acid-200"
                            : "border-white/10 bg-white/5 text-slate-500",
                        )}
                      >
                        {server.enabled ? "Enabled" : "Disabled"}
                      </button>
                      <button
                        type="button"
                        onClick={() => void probe(server)}
                        disabled={probing === server.id}
                        className="inline-flex h-6 items-center gap-1 rounded-md border border-white/10 bg-white/5 px-1.5 text-[9px] text-slate-300 transition hover:bg-white/10 disabled:opacity-40"
                      >
                        {probing === server.id ? (
                          <Loader2 className="h-2.5 w-2.5 animate-spin" />
                        ) : (
                          <Play className="h-2.5 w-2.5" />
                        )}
                        Probe
                      </button>
                      <button
                        type="button"
                        onClick={() => {
                          setServers((previous) =>
                            previous.filter(
                              (candidate) => candidate.id !== server.id,
                            ),
                          );
                          setProbes((previous) => {
                            const next = { ...previous };
                            delete next[server.id];
                            return next;
                          });
                        }}
                        className="rounded-md p-1 text-slate-600 transition hover:text-rose-300"
                        aria-label="Remove server"
                      >
                        <Trash2 className="h-3 w-3" />
                      </button>
                    </div>
                  </div>

                  {server.cwd || envNames(server.env).length ? (
                    <div className="mt-1 flex flex-wrap gap-x-3 gap-y-1 font-mono text-[9px] text-slate-700">
                      {server.cwd ? <span>cwd: {server.cwd}</span> : null}
                      {envNames(server.env).length ? (
                        <span>
                          env: {envNames(server.env).join(", ")}
                        </span>
                      ) : null}
                    </div>
                  ) : null}

                  {result ? (
                    <div className="mt-2 rounded-lg border border-white/5 bg-ink-950/60 px-3 py-2">
                      <div className="flex items-center gap-1.5 text-[10px]">
                        {result.ok ? (
                          <CheckCircle2 className="h-3 w-3 text-emerald-400" />
                        ) : (
                          <XCircle className="h-3 w-3 text-rose-400" />
                        )}
                        <span className="text-slate-300">{result.message}</span>
                        <span className="ml-auto font-mono text-[9px] text-slate-600">
                          {result.server_name}
                          {result.server_version
                            ? ` v${result.server_version}`
                            : ""}{" "}
                          · {result.elapsed_ms} ms
                        </span>
                      </div>
                      {result.tools.length ? (
                        <div className="mt-2 flex flex-wrap gap-1">
                          {result.tools.map((tool) => (
                            <span
                              key={tool.name}
                              title={tool.description}
                              className="rounded border border-white/10 bg-white/5 px-1.5 py-0.5 font-mono text-[9px] text-slate-400"
                            >
                              {tool.name}
                            </span>
                          ))}
                        </div>
                      ) : null}
                      <details className="mt-2" open={!result.ok}>
                        <summary className="cursor-pointer text-[9px] text-slate-600 hover:text-slate-400">
                          handshake log
                        </summary>
                        <pre className="mt-1 max-h-32 overflow-auto whitespace-pre-wrap font-mono text-[8.5px] leading-4 text-slate-600">
                          {result.log.join("\n")}
                        </pre>
                      </details>
                    </div>
                  ) : null}
                </div>
              );
            })}
          </div>
        )}
      </div>

      <div className="shrink-0 border-t border-white/5 bg-ink-900/40">
        <div className="flex items-center gap-2 px-6 py-2">
          <Terminal className="h-3 w-3 text-slate-600" />
          <span className="text-[9px] uppercase tracking-[0.16em] text-slate-500">
            Tool activity
          </span>
          <span className="text-[9px] text-slate-600">{log.length} events</span>
          <button
            type="button"
            onClick={() => {
              clearMcpLog();
              setLog([]);
            }}
            className="ml-auto text-[9px] text-slate-600 transition hover:text-slate-300"
          >
            Clear log
          </button>
        </div>
        <div className="max-h-32 overflow-y-auto px-6 pb-3">
          {log.length ? (
            log.map((entry) => (
              <p
                key={entry.id}
                className="flex gap-2 border-t border-white/5 py-1 font-mono text-[9px] text-slate-600 first:border-0"
              >
                <span className="shrink-0 text-slate-700">
                  {new Date(entry.at).toLocaleTimeString([], {
                    hour: "2-digit",
                    minute: "2-digit",
                    second: "2-digit",
                  })}
                </span>
                <span className="shrink-0 text-slate-500">{entry.server}</span>
                <span
                  className={cn(
                    "min-w-0 flex-1 truncate",
                    entry.kind === "error" ? "text-rose-300/80" : "text-slate-500",
                  )}
                >
                  {entry.message}
                </span>
              </p>
            ))
          ) : (
            <p className="py-2 text-[9px] text-slate-700">
              Tool handshakes and calls appear here, and in the chat drawer.
            </p>
          )}
        </div>
      </div>
    </div>
  );
}
