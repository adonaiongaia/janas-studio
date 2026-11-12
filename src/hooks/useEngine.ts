import { useCallback, useEffect, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import { Events } from "../lib/api";
import { engineController } from "../lib/engine-controller";
import { errorMessage } from "../lib/format";
import type {
  ActiveEngineInfo,
  AdapterWarningEvent,
  EngineKind,
  EngineSnapshot,
  EngineTelemetry,
} from "../types";

const INITIAL: EngineSnapshot = {
  status: "disconnected",
  detail: "Checking engine state…",
  pid: null,
  model: null,
  port: 8080,
  started_at_ms: null,
};

export interface EngineController {
  snapshot: EngineSnapshot;
  active: ActiveEngineInfo | null;
  telemetry: EngineTelemetry | null;
  adapterWarning: AdapterWarningEvent | null;
  busy: boolean;
  error: string | null;
  start: () => Promise<void>;
  stop: () => Promise<void>;
  switchTo: (kind: EngineKind | "none") => Promise<void>;
  refresh: () => Promise<void>;
  clearError: () => void;
  clearAdapterWarning: () => void;
}

export function useEngine(): EngineController {
  const [snapshot, setSnapshot] = useState<EngineSnapshot>(INITIAL);
  const [active, setActive] = useState<ActiveEngineInfo | null>(null);
  const [telemetry, setTelemetry] = useState<EngineTelemetry | null>(null);
  const [adapterWarning, setAdapterWarning] =
    useState<AdapterWarningEvent | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [nextSnapshot, nextActive] = await Promise.all([
        engineController.currentStatus(),
        engineController.current(),
      ]);
      setSnapshot(nextSnapshot);
      setActive(nextActive);
    } catch (err) {
      setError(errorMessage(err));
    }
  }, []);

  useEffect(() => {
    let disposed = false;
    const unlisteners: UnlistenFn[] = [];

    const attach = async () => {
      const statusUnlisten = await listen<EngineSnapshot>(
        Events.engineStatus,
        (event) => setSnapshot(event.payload),
      );
      const activeUnlisten = await listen<ActiveEngineInfo | null>(
        Events.engineActive,
        (event) => {
          setActive(event.payload);
          if (!event.payload) setTelemetry(null);
        },
      );
      const telemetryUnlisten = await listen<EngineTelemetry>(
        Events.engineTelemetry,
        (event) => setTelemetry(event.payload),
      );
      const warningUnlisten = await listen<AdapterWarningEvent>(
        Events.engineAdapterWarning,
        (event) => setAdapterWarning(event.payload),
      );
      const exitUnlisten = await listen(Events.engineExited, () => {
        void Promise.all([
          engineController.currentStatus(),
          engineController.current(),
        ])
          .then(([nextSnapshot, nextActive]) => {
            setSnapshot(nextSnapshot);
            setActive(nextActive);
          })
          .catch(() => undefined);
      });
      if (disposed) {
        statusUnlisten();
        activeUnlisten();
        telemetryUnlisten();
        warningUnlisten();
        exitUnlisten();
      } else {
        unlisteners.push(
          statusUnlisten,
          activeUnlisten,
          telemetryUnlisten,
          warningUnlisten,
          exitUnlisten,
        );
      }
    };

    void attach().catch(() => undefined);
    void refresh();

    return () => {
      disposed = true;
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, [refresh]);

  const start = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      setSnapshot(await engineController.start());
      setActive(await engineController.current());
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  }, []);

  const stop = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      await engineController.stop();
      setSnapshot(await engineController.currentStatus());
      setActive(null);
      setTelemetry(null);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  }, []);

  const switchTo = useCallback(async (kind: EngineKind | "none") => {
    setBusy(true);
    setError(null);
    try {
      setSnapshot(await engineController.switchTo(kind));
      setActive(await engineController.current());
    } catch (err) {
      setError(errorMessage(err));
      await refresh();
    } finally {
      setBusy(false);
    }
  }, [refresh]);

  const clearError = useCallback(() => setError(null), []);
  const clearAdapterWarning = useCallback(() => setAdapterWarning(null), []);

  return {
    snapshot,
    active,
    telemetry,
    adapterWarning,
    busy,
    error,
    start,
    stop,
    switchTo,
    refresh,
    clearError,
    clearAdapterWarning,
  };
}
