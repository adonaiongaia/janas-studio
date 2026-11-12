import { invoke } from "@tauri-apps/api/core";

import type { BackendCapabilities } from "../types";

export const SAMPLING_PARAMETERS = [
  { key: "temperature", label: "Temperature" },
  { key: "top_p", label: "Top-P" },
  { key: "top_k", label: "Top-K" },
  { key: "min_p", label: "Min-P" },
  { key: "repeat_penalty", label: "Repeat penalty" },
  { key: "max_tokens", label: "Max reply" },
] as const;

export type SamplingParameterKey = (typeof SAMPLING_PARAMETERS)[number]["key"];

/** The same safe fallback the Rust command returns when the engine is down. */
export const FALLBACK_BACKEND_CAPABILITIES: BackendCapabilities = {
  temperature: true,
  top_p: true,
  top_k: true,
  min_p: false,
  repeat_penalty: true,
  max_tokens: true,
};

function normalizeCapabilities(raw: unknown): BackendCapabilities {
  if (!raw || typeof raw !== "object") {
    return { ...FALLBACK_BACKEND_CAPABILITIES };
  }
  const source = raw as Record<string, unknown>;
  return Object.fromEntries(
    SAMPLING_PARAMETERS.map(({ key }) => [
      key,
      typeof source[key] === "boolean" ? source[key] : false,
    ]),
  ) as BackendCapabilities;
}

/** Call the Rust probe. A failed IPC call must never leave the UI without a
 * usable sampling panel, so it degrades to the same safe fallback map. */
export async function fetchBackendCapabilities(): Promise<BackendCapabilities> {
  try {
    return normalizeCapabilities(
      await invoke<Record<string, boolean>>("get_backend_capabilities"),
    );
  } catch {
    return { ...FALLBACK_BACKEND_CAPABILITIES };
  }
}

/** Pure DOM pass used by the AppImage frontend and available to any future
 * non-React shell. It hides the whole field group, so no disabled thumb or
 * leftover padding remains in the panel's vertical rhythm. */
export function applyBackendCapabilities(
  capabilities: BackendCapabilities,
  root: ParentNode = document,
): void {
  root.querySelectorAll<HTMLElement>("[data-param]").forEach((element) => {
    const parameter = element.dataset.param;
    if (!parameter) return;
    const hidden = capabilities[parameter] !== true;
    element.classList.toggle("hidden-param", hidden);
    element.setAttribute("aria-hidden", hidden ? "true" : "false");
    if (hidden) element.setAttribute("inert", "");
    else element.removeAttribute("inert");
  });
}
