import { useCallback, useEffect, useRef, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import { api, Events } from "./api";
import { errorMessage } from "./format";
import type { MemoryFact } from "../types";

export type { MemoryFact } from "../types";

/**
 * A small local vector store. Facts are embedded with a hashed bag-of-words
 * vector (TF-IDF weights) and recalled by cosine similarity. The facts
 * themselves live in the Rust memory bank on disk, not in localStorage.
 */
const DIMENSIONS = 512;
const LEGACY_KEY = "janas.memory.v1";

function tokenize(text: string): string[] {
  return text
    .toLowerCase()
    .replace(/[^a-z0-9_./-]+/g, " ")
    .split(/\s+/)
    .filter((token) => token.length > 1);
}

function embed(text: string, idf: Map<string, number>): Float32Array {
  const vector = new Float32Array(DIMENSIONS);
  const tokens = tokenize(text);
  for (const token of tokens) {
    let hash = 2166136261;
    for (let index = 0; index < token.length; index += 1) {
      hash ^= token.charCodeAt(index);
      hash = Math.imul(hash, 16777619);
    }
    const slot = Math.abs(hash) % DIMENSIONS;
    vector[slot] += (idf.get(token) ?? 1) * (token.length > 5 ? 1.4 : 1);
  }
  let norm = 0;
  for (let index = 0; index < DIMENSIONS; index += 1) {
    norm += vector[index] * vector[index];
  }
  const length = Math.sqrt(norm) || 1;
  for (let index = 0; index < DIMENSIONS; index += 1) {
    vector[index] /= length;
  }
  return vector;
}

function buildIdf(facts: MemoryFact[]): Map<string, number> {
  const frequency = new Map<string, number>();
  for (const fact of facts) {
    for (const token of new Set(tokenize(fact.text))) {
      frequency.set(token, (frequency.get(token) ?? 0) + 1);
    }
  }
  const total = Math.max(1, facts.length);
  const idf = new Map<string, number>();
  for (const [token, count] of frequency) {
    idf.set(token, Math.log((total + 1) / (count + 1)) + 1);
  }
  return idf;
}

export function recallFacts(
  facts: MemoryFact[],
  query: string,
  limit = 6,
): MemoryFact[] {
  if (!facts.length || !query.trim()) return [];
  const idf = buildIdf(facts);
  const queryVector = embed(query, idf);
  return facts
    .map((fact) => {
      const vector = embed(fact.text, idf);
      let score = 0;
      for (let index = 0; index < DIMENSIONS; index += 1) {
        score += queryVector[index] * vector[index];
      }
      return { fact, score };
    })
    .sort((a, b) => b.score - a.score)
    .filter((entry) => entry.score > 0.01)
    .slice(0, limit)
    .map((entry) => entry.fact);
}

export function useMemory() {
  const [facts, setFacts] = useState<MemoryFact[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const mounted = useRef(true);

  const refresh = useCallback(async () => {
    try {
      const next = await api.memoryList();
      if (!mounted.current) return;
      setFacts(Array.isArray(next) ? next : []);
      setError(null);
    } catch (err) {
      if (mounted.current) setError(errorMessage(err));
    }
  }, []);

  useEffect(() => {
    mounted.current = true;
    let disposed = false;
    let unlisten: UnlistenFn | undefined;

    const migrateLegacy = async () => {
      try {
        const raw = window.localStorage.getItem(LEGACY_KEY);
        if (!raw) return;
        const parsed: unknown = JSON.parse(raw);
        if (!Array.isArray(parsed)) {
          window.localStorage.removeItem(LEGACY_KEY);
          return;
        }
        let complete = true;
        for (const entry of parsed) {
          const text =
            typeof entry === "string"
              ? entry
              : (entry as { text?: unknown } | null)?.text;
          if (typeof text !== "string" || !text.trim()) continue;
          try {
            await api.saveToMemoryBank(text.trim());
          } catch {
            complete = false;
          }
        }
        // Keep the old copy if the disk bank was unavailable, so a later launch
        // can try again instead of losing the user's notes.
        if (complete) window.localStorage.removeItem(LEGACY_KEY);
      } catch {
        // Leave malformed legacy data untouched.
      }
    };

    const attach = async () => {
      const stop = await listen<MemoryFact[]>(Events.memoryChanged, (event) => {
        if (disposed) return;
        setFacts(Array.isArray(event.payload) ? event.payload : []);
        setError(null);
      });
      if (disposed) stop();
      else unlisten = stop;
    };

    void attach().catch((err) => {
      if (!disposed) {
        setError(`Memory updates could not be connected: ${errorMessage(err)}`);
      }
    });
    void (async () => {
      await migrateLegacy();
      if (!disposed) await refresh();
      if (!disposed) setLoading(false);
    })();

    return () => {
      disposed = true;
      mounted.current = false;
      unlisten?.();
    };
  }, [refresh]);

  const add = useCallback(async (text: string) => {
    try {
      const fact = await api.saveToMemoryBank(text);
      setFacts((previous) => [
        fact,
        ...previous.filter((candidate) => candidate.id !== fact.id),
      ]);
      setError(null);
    } catch (err) {
      setError(errorMessage(err));
    }
  }, []);

  const remove = useCallback(async (id: string) => {
    try {
      const removed = await api.memoryDelete(id);
      if (removed) {
        setFacts((previous) => previous.filter((fact) => fact.id !== id));
      }
      setError(null);
    } catch (err) {
      setError(errorMessage(err));
    }
  }, []);

  const edit = useCallback(async (id: string, text: string) => {
    try {
      const updated = await api.memoryUpdate(id, text);
      setFacts((previous) =>
        previous.map((fact) => (fact.id === id ? updated : fact)),
      );
      setError(null);
    } catch (err) {
      setError(errorMessage(err));
    }
  }, []);

  const clear = useCallback(async () => {
    try {
      await api.memoryClear();
      setFacts([]);
      setError(null);
    } catch (err) {
      setError(errorMessage(err));
    }
  }, []);

  return { facts, loading, error, add, remove, edit, clear };
}

export type MemoryController = ReturnType<typeof useMemory>;
