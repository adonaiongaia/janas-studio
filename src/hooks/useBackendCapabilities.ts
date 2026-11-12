import { useEffect, useLayoutEffect, useState } from "react";

import {
  applyBackendCapabilities,
  FALLBACK_BACKEND_CAPABILITIES,
  fetchBackendCapabilities,
} from "../lib/backendCapabilities";
import type { BackendCapabilities } from "../types";

/** Refresh whenever the active engine or its readiness changes. The layout
 * effect is intentionally dependency-free: React can re-render the panel (for
 * example, when the token ticker updates) and restore a wrapper's className,
 * so the vanilla DOM pass has to run after every commit. */
export function useBackendCapabilities(refreshKey: string) {
  const [capabilities, setCapabilities] = useState<BackendCapabilities>(
    FALLBACK_BACKEND_CAPABILITIES,
  );

  useEffect(() => {
    let disposed = false;
    void fetchBackendCapabilities().then((next) => {
      if (!disposed) setCapabilities(next);
    });
    return () => {
      disposed = true;
    };
  }, [refreshKey]);

  useLayoutEffect(() => {
    applyBackendCapabilities(capabilities);
  });

  return capabilities;
}
