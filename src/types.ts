export type ViewId =
  | "chat"
  | "models"
  | "library"
  | "compiler"
  | "mcp"
  | "settings";

export type EngineStatus = "disconnected" | "indexing" | "loading_model" | "ready";
export type EngineKind = "janas" | "llama";

export interface EngineSnapshot {
  status: EngineStatus;
  detail: string;
  pid: number | null;
  model: string | null;
  port: number;
  started_at_ms: number | null;
}

export interface EngineLog {
  stream: "stdout" | "stderr" | string;
  line: string;
  timestamp_ms: number;
}

export interface ActiveEngineInfo {
  kind: EngineKind;
  pid: number;
  host: string;
  port: number;
  model: string;
  context_window: number;
  version: string | null;
  adapter: string;
  warning: string | null;
  started_at_ms: number;
}

export interface EngineTelemetry {
  engine: EngineKind;
  pid: number | null;
  tokens_per_second: number;
  prompt_tokens_total: number;
  completion_tokens_total: number;
  kv_cache_usage: number;
  read_bytes_per_sec: number;
  read_ops_per_sec: number;
  rss_bytes: number;
  sampled_at_ms: number;
}

export interface AdapterWarningEvent {
  engine: EngineKind;
  message: string;
  version: string | null;
  adapter: string;
}

export interface BinaryProgress {
  engine: EngineKind;
  phase: string;
  downloaded: number;
  total: number | null;
  percent: number | null;
  message: string;
}

export interface BinaryInstallResult {
  engine: EngineKind;
  installed_path: string;
  backup_path: string | null;
  version: string | null;
  message: string;
  rolled_back: boolean;
}

export interface ManagedBinaryProbe {
  engine: EngineKind;
  installed: boolean;
  path: string;
  version: string | null;
  build: number | null;
  message: string;
}

export type SetupStatus = "idle" | "running" | "ready" | "degraded" | "failed";
export type SetupComponentStatus =
  | "pending"
  | "present"
  | "installed"
  | "skipped"
  | "failed";

export interface SetupGpu {
  vendor: string;
  backend: string;
  name: string;
  cuda_version: string | null;
}

export interface SetupPlatform {
  os: string;
  arch: string;
  cpu_avx2: boolean;
  apple_silicon: boolean;
  gpus: SetupGpu[];
  preferred_backend: string;
}

export interface SetupRelease {
  repo: string;
  requested_tag: string;
  tag: string;
  asset: string;
  download_url: string;
  fallback: boolean;
}

export interface SetupBinary {
  name: string;
  status: SetupComponentStatus;
  path: string | null;
  version: string | null;
  source: string | null;
  message: string;
}

export interface SetupReport {
  status: SetupStatus;
  message: string;
  started_at_ms: number | null;
  finished_at_ms: number | null;
  platform: SetupPlatform | null;
  cache_dir: string | null;
  llama_release: SetupRelease | null;
  janas_release: SetupRelease | null;
  binaries: SetupBinary[];
  warnings: string[];
}

export interface AppSettings {
  engine_path: string;
  converter_path: string;
  host: string;
  port: number;
  models_dir: string;
  model_path: string;
  extra_args: string;
  api_key: string;
  /** --ctx: token window the server allocates. */
  ctx_size: number;
  /** --cache: GiB reserved for streamed experts; 0 is automatic. */
  cache_gib: number;
  /** --mtp / --no-mtp: "auto" | "off" | "file". */
  mtp_mode: "auto" | "off" | "file" | string;
  mtp_path: string;
  draft_path: string;
  prompt_on: string;
  reply_on: string;
  drafts_on: string;
  think: "auto" | "on" | "off" | string;
  npu_int8: boolean;
  engine_mode: string;
  /** JANAS_ATTN: auto, exact (float) or fast (16-bit query). */
  janas_attention: "auto" | "exact" | "fast" | string;
  /** JANAS_KV: auto, 8 or 16 bits for both keys and values. */
  janas_kv_cache: "auto" | "8" | "16" | string;
  llama_model_path: string;
  llama_mmproj_path: string;
  llama_port: number;
  llama_gpu_layers: number;
  llama_ctx_size: number;
  llama_flash_attn: "auto" | "on" | "off" | string;
  llama_kv_cache_k: string;
  llama_kv_cache_v: string;
  /** llama.cpp only: execute MoE cache misses on host CPU experts. */
  llama_direct_cpu_moe: boolean;
  /** llama.cpp only: max CPU threads for direct MoE compute; 0 = default. */
  llama_cpu_threads: number;
  llama_extra_args: string;
  ssd_streaming: boolean;
  ssd_cache_dir: string;
  top_k: number;
}

export interface BinaryProbe {
  ok: boolean;
  resolved_path: string | null;
  version: string | null;
  message: string;
}

export interface LocalModel {
  name: string;
  path: string;
  size_bytes: number;
}

export interface GgufModelInfo {
  path: string;
  architecture: string | null;
  layer_count: number | null;
}

export interface ChatUsage {
  prompt_tokens?: number;
  completion_tokens?: number;
  total_tokens?: number;
  janas?: Record<string, unknown>;
  [key: string]: unknown;
}

export interface Citation {
  title: string;
  url: string;
  snippet?: string;
}

export interface MemoryFact {
  id: string;
  text: string;
  created_at: number;
  source: string;
}

export interface ChatMessage {
  id: string;
  role: "user" | "assistant";
  content: string;
  reasoning: string;
  created_at: number;
  citations?: Citation[];
  images?: Array<{ name: string; data_url: string; tokens: number }>;
}

export interface ChatSession {
  id: string;
  title: string;
  created_at: number;
  updated_at: number;
  messages: ChatMessage[];
}

export interface ChatOutboundMessage {
  role: "user" | "assistant";
  content: string;
}

export interface ChatRequestPayload {
  request_id: string;
  messages: ChatOutboundMessage[];
  system_prompt: string | null;
  temperature: number;
  top_p: number;
  top_k: number;
  min_p: number | null;
  repeat_penalty: number | null;
  structured: boolean;
  thinking: boolean | null;
  images: string[];
  max_tokens: number | null;
  mcp_servers: McpServerInput[];
}

export interface ChatDelta {
  request_id: string;
  delta: string;
  reasoning: string;
}

export interface ChatDone {
  request_id: string;
  finish_reason: string;
  usage: ChatUsage | null;
  elapsed_ms: number;
  first_token_ms: number | null;
  cancelled: boolean;
}

export interface ChatErrorEvent {
  request_id: string;
  message: string;
}

export interface ChatNotice {
  request_id: string;
  message: string;
}

export interface HfFile {
  path: string;
  size: number;
}

export interface HfQuant {
  quant: string;
  files: HfFile[];
  size_bytes: number;
}

export interface HfModel {
  id: string;
  author: string;
  name: string;
  downloads: number;
  likes: number;
  pipeline_tag: string | null;
  tags: string[];
  parameters: number | null;
  last_modified: string | null;
  quants: HfQuant[];
}

export interface SearchRequest {
  query: string;
  gguf_only: boolean;
  limit: number;
}

export interface DownloadFile {
  path: string;
  size: number;
}

export interface DownloadRequest {
  repo: string;
  files: DownloadFile[];
}

export interface DownloadStarted {
  id: string;
}

export type DownloadStatus =
  | "preparing"
  | "downloading"
  | "paused"
  | "complete"
  | "cancelled"
  | "error";

export interface DownloadProgress {
  id: string;
  repo: string;
  file: string;
  downloaded: number;
  total: number;
  percent: number | null;
  files_done: number;
  files_total: number;
  status: DownloadStatus;
  message: string;
  saved_path: string | null;
}

export interface ConvertRequest {
  input: string;
  output: string;
  overwrite: boolean;
}

export interface ConversionStarted {
  run_id: string;
  command: string;
  converter_path: string;
  output_path: string;
}

export interface ConverterLog {
  run_id: string;
  stream: "stdout" | "stderr" | "system" | string;
  line: string;
  timestamp_ms: number;
}

export interface ConverterProgress {
  run_id: string;
  percent: number | null;
}

export interface ConversionFinished {
  run_id: string;
  success: boolean;
  code: number | null;
  elapsed_ms: number;
  output_path: string;
  output_size: number;
  message: string;
}

/** Everything the Engine Settings panel sends with a chat request, persisted
 * locally. Server-launch options live in AppSettings instead. */
export interface EnginePreferences {
  temperature: number;
  topP: number;
  /** llama.cpp / Janas top-k cut-off, sent with every request. */
  topK: number;
  minP: number;
  repeatPenalty: number;
  maxTokens: number | null;
  thinking: boolean;
  structuredOutput: boolean;
  /** How many previous turns are re-sent as context. */
  contextPrefetch: number;
  eviction: "lru" | "lfu";
  internet: boolean;
}

export interface WebPageResult {
  title: string;
  url: string;
  snippet: string;
  text: string;
  bytes: number;
}

export interface WebSearchResponse {
  query: string;
  provider: string;
  results: WebPageResult[];
  elapsed_ms: number;
  bytes: number;
  message: string;
}

export interface DeviceMetrics {
  present: boolean;
  name: string;
  utilization: number | null;
  memory_used: number | null;
  memory_total: number | null;
}

export interface Telemetry {
  pid: number | null;
  process_rss_bytes: number;
  process_cpu_percent: number;
  read_bytes_per_sec: number;
  read_ops_per_sec: number;
  model_bytes: number;
  model_name: string;
  resident_ratio: number;
  cores: number;
  mem_total_bytes: number;
  mem_available_bytes: number;
  gpu: DeviceMetrics;
  npu: DeviceMetrics;
  sampled_at_ms: number;
}

export interface StorageBenchmark {
  path: string;
  file_bytes: number;
  sequential_mb_s: number;
  random_read_iops: number;
  random_read_mb_s: number;
  average_latency_us: number;
  elapsed_ms: number;
  advice: string;
}

export interface ProjectIngest {
  root: string;
  name: string;
  files: number;
  bytes: number;
  tokens: number;
  tree: string;
  bundle: string;
  truncated: boolean;
}

export interface LibraryModel {
  name: string;
  path: string;
  format: string;
  size_bytes: number;
  quant: string;
  architecture: string;
  parameters: string;
  publisher: string;
  repo: string | null;
  downloaded_at: string | null;
  thinking: boolean;
  tools: boolean;
  capabilities_source: string;
}

/** Backend-advertised sampling controls. Missing keys are treated as false. */
export type BackendCapabilities = Record<string, boolean>;

export interface McpServerConfig {
  id: string;
  name: string;
  command: string;
  args: string[];
  enabled: boolean;
  /** One KEY=VALUE per line. */
  env: string;
  cwd: string;
}

export interface McpServerInput {
  name: string;
  command: string;
  args: string[];
  env: string;
  cwd: string;
}

export interface McpTool {
  name: string;
  description: string;
}

export interface McpProbe {
  ok: boolean;
  server_name: string;
  server_version: string;
  tools: McpTool[];
  log: string[];
  message: string;
  elapsed_ms: number;
}

export interface McpActivityEvent {
  server: string;
  tool: string;
  level: "info" | "error" | string;
  message: string;
}
