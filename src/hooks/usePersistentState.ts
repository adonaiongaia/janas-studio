import { useEffect, useState } from "react";

/**
 * Persists small UI preferences (system prompt, sliders) locally. These are not
 * secrets and never leave the machine.
 */
export function usePersistentState<T>(key: string, initial: T) {
  const [value, setValue] = useState<T>(() => {
    try {
      const stored = window.localStorage.getItem(key);
      return stored ? (JSON.parse(stored) as T) : initial;
    } catch {
      return initial;
    }
  });

  useEffect(() => {
    try {
      window.localStorage.setItem(key, JSON.stringify(value));
    } catch {
      // Storage may be disabled; the app still works for this session.
    }
  }, [key, value]);

  return [value, setValue] as const;
}
