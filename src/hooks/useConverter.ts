import { useCallback, useEffect, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import { api, Events } from "../lib/api";
import { errorMessage } from "../lib/format";
import type {
  ConversionFinished,
  ConversionStarted,
  ConverterLog,
  ConverterProgress,
  ConvertRequest,
} from "../types";

const MAX_LOG_LINES = 3_000;

export function useConverter() {
  const [logs, setLogs] = useState<ConverterLog[]>([]);
  const [progress, setProgress] = useState<number | null>(null);
  const [started, setStarted] = useState<ConversionStarted | null>(null);
  const [finished, setFinished] = useState<ConversionFinished | null>(null);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let disposed = false;
    const unlisteners: UnlistenFn[] = [];

    const attach = async () => {
      const startedUnlisten = await listen<ConversionStarted>(
        Events.convertStarted,
        (event) => setStarted(event.payload),
      );
      const logUnlisten = await listen<ConverterLog>(
        Events.convertLog,
        (event) => {
          setLogs((previous) => {
            const next = [...previous, event.payload];
            return next.length > MAX_LOG_LINES
              ? next.slice(next.length - MAX_LOG_LINES)
              : next;
          });
        },
      );
      const progressUnlisten = await listen<ConverterProgress>(
        Events.convertProgress,
        (event) => setProgress(event.payload.percent),
      );
      const finishedUnlisten = await listen<ConversionFinished>(
        Events.convertFinished,
        (event) => {
          setFinished(event.payload);
          setRunning(false);
          if (event.payload.success) setProgress(100);
        },
      );

      if (disposed) {
        startedUnlisten();
        logUnlisten();
        progressUnlisten();
        finishedUnlisten();
      } else {
        unlisteners.push(
          startedUnlisten,
          logUnlisten,
          progressUnlisten,
          finishedUnlisten,
        );
      }
    };

    void attach().catch(() => undefined);
    return () => {
      disposed = true;
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, []);

  const start = useCallback(async (request: ConvertRequest) => {
    setLogs([]);
    setProgress(null);
    setStarted(null);
    setFinished(null);
    setError(null);
    setRunning(true);
    try {
      setStarted(await api.startConversion(request));
    } catch (err) {
      setError(errorMessage(err));
      setRunning(false);
    }
  }, []);

  const cancel = useCallback(async () => {
    try {
      await api.cancelConversion();
    } catch (err) {
      setError(errorMessage(err));
    }
  }, []);

  const clear = useCallback(() => {
    if (running) return;
    setLogs([]);
    setProgress(null);
    setFinished(null);
    setError(null);
    setStarted(null);
  }, [running]);

  return {
    logs,
    progress,
    started,
    finished,
    running,
    error,
    start,
    cancel,
    clear,
  };
}

export type ConverterController = ReturnType<typeof useConverter>;
