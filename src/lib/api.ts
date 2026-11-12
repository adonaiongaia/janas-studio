import { invoke } from "@tauri-apps/api/core";

import type {
  ActiveEngineInfo,
  AppSettings,
  BackendCapabilities,
  BinaryInstallResult,
  BinaryProbe,
  ChatRequestPayload,
  EngineKind,
  ManagedBinaryProbe,
  ConversionStarted,
  ConvertRequest,
  DownloadRequest,
  DownloadStarted,
  EngineSnapshot,
  GgufModelInfo,
  HfModel,
  HfQuant,
  LibraryModel,
  LocalModel,
  McpProbe,
  McpServerInput,
  MemoryFact,
  ProjectIngest,
  SearchRequest,
  SetupReport,
  StorageBenchmark,
  Telemetry,
  WebSearchResponse,
} from "../types";

export const Events = {
  engineStatus: "engine://status",
  engineLog: "engine://log",
  engineExited: "engine://exited",
  engineActive: "engine://active",
  engineTelemetry: "engine://telemetry",
  engineAdapterWarning: "engine://adapter-warning",
  engineBinaryProgress: "engine://binary-progress",
  engineSetupStatus: "setup://status",
  chatDelta: "chat://delta",
  chatDone: "chat://done",
  chatError: "chat://error",
  chatNotice: "chat://notice",
  memoryChanged: "memory://changed",
  convertStarted: "convert://started",
  convertLog: "convert://log",
  convertProgress: "convert://progress",
  convertFinished: "convert://finished",
  downloadProgress: "download://progress",
  mcpActivity: "mcp://activity",
} as const;

export const api = {
  getSettings: () => invoke<AppSettings>("get_settings"),
  saveSettings: (next: AppSettings) =>
    invoke<AppSettings>("save_settings", { next }),
  probeBinary: (path: string) =>
    invoke<BinaryProbe>("probe_binary", { path }),
  scanLocalModels: () => invoke<LocalModel[]>("scan_local_models"),
  inspectGgufModel: (path: string) =>
    invoke<GgufModelInfo>("inspect_gguf_model", { path }),

  getEngineStatus: () => invoke<EngineSnapshot>("get_engine_status"),
  getActiveEngine: () => invoke<ActiveEngineInfo | null>("get_active_engine"),
  getBackendCapabilities: () =>
    invoke<BackendCapabilities>("get_backend_capabilities"),
  startEngine: () => invoke<EngineSnapshot>("start_engine"),
  stopEngine: () => invoke<void>("stop_engine"),
  switchEngine: (engine: EngineKind | "none") =>
    invoke<EngineSnapshot>("switch_engine", { engine }),
  downloadAndExtractBinary: (engineType: EngineKind, downloadUrl: string) =>
    invoke<BinaryInstallResult>("download_and_extract_binary", {
      engine_type: engineType,
      download_url: downloadUrl,
    }),
  rollbackBinary: (engineType: EngineKind) =>
    invoke<BinaryInstallResult>("rollback_binary", {
      engine_type: engineType,
    }),
  probeManagedBinary: (engineType: EngineKind) =>
    invoke<ManagedBinaryProbe>("probe_managed_binary", {
      engine_type: engineType,
    }),
  getEngineSetupStatus: () =>
    invoke<SetupReport>("get_engine_setup_status"),
  rerunEngineSetup: () =>
    invoke<SetupReport>("rerun_engine_setup"),

  streamChat: (request: ChatRequestPayload) =>
    invoke<void>("stream_chat", { request }),
  cancelChat: (requestId: string) =>
    invoke<void>("cancel_chat", { request_id: requestId }),

  startConversion: (request: ConvertRequest) =>
    invoke<ConversionStarted>("start_conversion", { request }),
  cancelConversion: () => invoke<void>("cancel_conversion"),

  searchModels: (request: SearchRequest) =>
    invoke<HfModel[]>("search_models", { request }),
  listModelQuants: (repo: string) =>
    invoke<HfQuant[]>("list_model_quants", { repo }),
  startDownload: (request: DownloadRequest) =>
    invoke<DownloadStarted>("start_download", { request }),
  cancelDownload: (id: string) => invoke<boolean>("cancel_download", { id }),
  pauseDownload: (id: string) => invoke<boolean>("pause_download", { id }),
  resumeDownload: (id: string) => invoke<boolean>("resume_download", { id }),

  memoryList: () => invoke<MemoryFact[]>("memory_list"),
  saveToMemoryBank: (fact: string) =>
    invoke<MemoryFact>("save_to_memory_bank", { fact }),
  memoryUpdate: (id: string, text: string) =>
    invoke<MemoryFact>("memory_update", { id, text }),
  memoryDelete: (id: string) => invoke<boolean>("memory_delete", { id }),
  memoryClear: () => invoke<void>("memory_clear"),
  openExternal: (url: string) => invoke<void>("open_external", { url }),

  getTelemetry: (pid: number | null) =>
    invoke<Telemetry>("get_telemetry", { pid }),
  benchmarkStorage: (directory?: string) =>
    invoke<StorageBenchmark>("benchmark_storage", {
      directory: directory ?? null,
    }),
  ingestProject: (path: string) =>
    invoke<ProjectIngest>("ingest_project", { path }),
  webSearch: (query: string, pages = 3) =>
    invoke<WebSearchResponse>("web_search", { query, pages }),
  transcribeAudio: (audioBase64: string, mime: string) =>
    invoke<string>("transcribe_audio", {
      audioBase64,
      mime,
    }),
  libraryModels: () => invoke<LibraryModel[]>("library_models"),
  mcpProbe: (server: McpServerInput) =>
    invoke<McpProbe>("mcp_probe", { server }),
};
