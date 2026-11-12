import { uid } from "./format";

export interface McpLogEntry {
  id: string;
  at: number;
  server: string;
  kind: "info" | "probe" | "tool" | "error";
  message: string;
}

const KEY = "janas.mcpLog.v1";
const EVENT = "janas:mcp-log";

export function readMcpLog(): McpLogEntry[] {
  try {
    const raw = window.localStorage.getItem(KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw) as McpLogEntry[];
    return Array.isArray(parsed) ? parsed.slice(0, 80) : [];
  } catch {
    return [];
  }
}

export function appendMcpLog(
  entry: Omit<McpLogEntry, "id" | "at"> & { at?: number },
): McpLogEntry {
  const full: McpLogEntry = {
    id: uid(),
    at: entry.at ?? Date.now(),
    server: entry.server,
    kind: entry.kind,
    message: entry.message,
  };
  try {
    const next = [full, ...readMcpLog()].slice(0, 80);
    window.localStorage.setItem(KEY, JSON.stringify(next));
    window.dispatchEvent(new CustomEvent(EVENT));
  } catch {
    // ignore storage failures
  }
  return full;
}

export function clearMcpLog() {
  try {
    window.localStorage.setItem(KEY, "[]");
    window.dispatchEvent(new CustomEvent(EVENT));
  } catch {
    // ignore
  }
}

export function subscribeMcpLog(listener: () => void): () => void {
  window.addEventListener(EVENT, listener);
  return () => window.removeEventListener(EVENT, listener);
}
