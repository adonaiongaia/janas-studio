import { useCallback, useEffect } from "react";

import { usePersistentState } from "./usePersistentState";
import type { EnginePreferences } from "../types";

export const DEFAULT_ENGINE_PREFERENCES: EnginePreferences = {
  temperature: 0.7,
  topP: 0.9,
  topK: 40,
  minP: 0.02,
  repeatPenalty: 1.08,
  maxTokens: null,
  thinking: true,
  structuredOutput: false,
  contextPrefetch: 12,
  eviction: "lru",
  internet: false,
};

/** The sampling and request-shaping knobs, kept per machine rather than per
 * chat so switching sessions never silently changes the model's behaviour. */
export function useEnginePreferences() {
  const [preferences, setPreferences] = usePersistentState<EnginePreferences>(
    "janas.enginePreferences.v2",
    DEFAULT_ENGINE_PREFERENCES,
  );

  // v2 files written before Top-K existed have no topK field. Fill it in once
  // so the slider and request payload always send an integer.
  useEffect(() => {
    if (typeof preferences.topK !== "number") {
      setPreferences((previous) => ({ ...previous, topK: 40 }));
    }
  }, [preferences.topK, setPreferences]);

  const update = useCallback(
    <K extends keyof EnginePreferences>(key: K, value: EnginePreferences[K]) => {
      setPreferences((previous) => ({ ...previous, [key]: value }));
    },
    [setPreferences],
  );

  return { preferences, update, setPreferences };
}
