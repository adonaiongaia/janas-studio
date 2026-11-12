import { useCallback, useEffect, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import { api, Events } from "../lib/api";
import type { DownloadProgress, DownloadRequest } from "../types";

export function useDownloads() {
  const [downloads, setDownloads] = useState<Record<string, DownloadProgress>>(
    {},
  );

  useEffect(() => {
    let disposed = false;
    let unlisten: UnlistenFn | undefined;

    void listen<DownloadProgress>(Events.downloadProgress, (event) => {
      setDownloads((previous) => ({
        ...previous,
        [event.payload.id]: event.payload,
      }));
    }).then((stopListening) => {
      if (disposed) stopListening();
      else unlisten = stopListening;
    });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const start = useCallback(async (request: DownloadRequest) => {
    const started = await api.startDownload(request);
    setDownloads((previous) => {
      if (previous[started.id]) return previous;
      return {
        ...previous,
        [started.id]: {
          id: started.id,
          repo: request.repo,
          file: request.files[0]?.path ?? "",
          downloaded: 0,
          total: request.files.reduce((sum, file) => sum + file.size, 0),
          percent: 0,
          files_done: 0,
          files_total: request.files.length,
          status: "preparing",
          message: "Queued",
          saved_path: null,
        },
      };
    });
    return started.id;
  }, []);

  const cancel = useCallback(async (id: string) => {
    await api.cancelDownload(id);
  }, []);

  const pause = useCallback(async (id: string) => {
    await api.pauseDownload(id);
  }, []);

  const resume = useCallback(async (id: string) => {
    await api.resumeDownload(id);
  }, []);

  const clearFinished = useCallback(() => {
    setDownloads((previous) =>
      Object.fromEntries(
        Object.entries(previous).filter(([, download]) =>
          ["preparing", "downloading", "paused"].includes(download.status),
        ),
      ),
    );
  }, []);

  return { downloads, start, cancel, pause, resume, clearFinished };
}

export type DownloadsController = ReturnType<typeof useDownloads>;
