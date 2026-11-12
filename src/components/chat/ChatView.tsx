import { useCallback, useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  AlertTriangle,
  Cpu,
  FolderOpen,
  Loader2,
  Play,
  Terminal,
  Trash2,
} from "lucide-react";

import type { ChatController } from "../../hooks/useChat";
import type { EngineController } from "../../hooks/useEngine";
import {
  DEFAULT_ENGINE_PREFERENCES,
  useEnginePreferences,
} from "../../hooks/useEnginePreferences";
import { usePersistentState } from "../../hooks/usePersistentState";
import { useSettings } from "../../hooks/useSettings";
import { useTelemetry } from "../../hooks/useTelemetry";
import { api } from "../../lib/api";
import { errorMessage, basename } from "../../lib/format";
import { recallFacts, useMemory } from "../../lib/memory";
import { buildWebContext, CORE_SYSTEM_PROMPT } from "../../lib/prompts";
import type {
  AppSettings,
  Citation,
  EngineKind,
  LibraryModel,
  LocalModel,
  McpServerConfig,
  ProjectIngest,
  StorageBenchmark,
} from "../../types";
import { Logo } from "../Logo";
import { Button } from "../ui/Button";
import { Composer, type ComposerImage } from "./Composer";
import { EnginePanel } from "./EnginePanel";
import { McpLogDrawer } from "./McpLogDrawer";
import { MessageBoundary } from "../MessageBoundary";
import { MessageBubble } from "./MessageBubble";
import { SessionsRail } from "./SessionsRail";
import { TelemetryPanel } from "./TelemetryPanel";

interface ChatViewProps {
  engine: EngineController;
  chat: ChatController;
}

const SUGGESTIONS = [
  {
    title: "Explain a trade-off",
    prompt:
      "Explain the trade-offs between keeping mixture-of-experts weights in RAM and streaming them from an NVMe SSD.",
  },
  {
    title: "Read my project",
    prompt:
      "Summarise the architecture of this project from the attached folder, then point out the riskiest module.",
  },
  {
    title: "Search and verify",
    prompt:
      "Search the web for the latest Janas release notes and summarise what changed, with sources.",
  },
];

export function ChatView({ engine, chat }: ChatViewProps) {
  const { settings, save } = useSettings();
  const { preferences, update, setPreferences } = useEnginePreferences();
  const memory = useMemory();
  const telemetry = useTelemetry(engine.snapshot.pid, true);

  const [systemPrompt, setSystemPrompt] = usePersistentState(
    "janas.systemPrompt",
    "",
  );
  const [mcpServers] = usePersistentState<McpServerConfig[]>(
    "janas.mcpServers.v1",
    [],
  );
  const [input, setInput] = useState("");
  const [models, setModels] = useState<LocalModel[]>([]);
  const [llamaModels, setLlamaModels] = useState<LibraryModel[]>([]);
  const [switchingModel, setSwitchingModel] = useState(false);
  const [modelError, setModelError] = useState<string | null>(null);
  const [sessionsCollapsed, setSessionsCollapsed] = usePersistentState(
    "janas.ui.sessionsCollapsed",
    false,
  );
  const [engineCollapsed, setEngineCollapsed] = usePersistentState(
    "janas.ui.engineCollapsed",
    false,
  );
  const [telemetryCollapsed, setTelemetryCollapsed] = usePersistentState(
    "janas.ui.telemetryCollapsed",
    false,
  );

  const [project, setProject] = useState<ProjectIngest | null>(null);
  const [projectBusy, setProjectBusy] = useState(false);
  const [activity, setActivity] = useState<string | null>(null);
  const [images, setImages] = useState<ComposerImage[]>([]);
  const [benchmark, setBenchmark] = useState<StorageBenchmark | null>(null);
  const [benchmarkBusy, setBenchmarkBusy] = useState(false);
  const [mcpOpen, setMcpOpen] = useState(false);
  const endRef = useRef<HTMLDivElement>(null);

  const ready = engine.snapshot.status === "ready";
  const activeKind = engine.active?.kind ?? null;
  const currentModelPath = settings?.model_path ?? "";
  const llamaModelPath = settings?.llama_model_path ?? "";
  const selectedModelPath =
    activeKind === "llama"
      ? llamaModelPath
      : activeKind === "janas"
        ? currentModelPath
        : "";
  const modelName = selectedModelPath
    ? basename(selectedModelPath)
    : activeKind === "llama"
      ? "No GGUF selected"
      : "No model selected";

  const loadModels = useCallback(async () => {
    try {
      setModels(await api.scanLocalModels());
    } catch {
      setModels([]);
    }
    try {
      const library = await api.libraryModels();
      setLlamaModels(
        library.filter((model) => model.path.toLowerCase().endsWith(".gguf")),
      );
    } catch {
      setLlamaModels([]);
    }
  }, []);

  useEffect(() => {
    void loadModels();
  }, [loadModels, currentModelPath, llamaModelPath]);

  useEffect(() => {
    endRef.current?.scrollIntoView({ block: "end" });
  }, [chat.messages, chat.streaming]);

  const chooseModel = useCallback(
    async (path: string) => {
      if (!settings || !path) return;
      setSwitchingModel(true);
      setModelError(null);
      try {
        await save({ ...settings, model_path: path });
        await loadModels();
        if (engine.snapshot.pid !== null) await engine.stop();
        await engine.switchTo("janas");
      } catch (err) {
        setModelError(errorMessage(err));
      } finally {
        setSwitchingModel(false);
      }
    },
    [engine, loadModels, save, settings],
  );

  const chooseLlamaModel = useCallback(
    async (path: string) => {
      if (!settings || !path) return;
      setSwitchingModel(true);
      setModelError(null);
      try {
        await save({ ...settings, llama_model_path: path });
        await loadModels();
        if (engine.snapshot.pid !== null) await engine.stop();
        await engine.switchTo("llama");
      } catch (err) {
        setModelError(errorMessage(err));
      } finally {
        setSwitchingModel(false);
      }
    },
    [engine, loadModels, save, settings],
  );

  const chooseAnyModel = useCallback(
    (path: string) => {
      if (!path) return;
      if (path.toLowerCase().endsWith(".gguf")) {
        void chooseLlamaModel(path);
      } else {
        void chooseModel(path);
      }
    },
    [chooseLlamaModel, chooseModel],
  );

  const attachProject = useCallback(
    async (path: string) => {
      setProjectBusy(true);
      setActivity("Parsing the project tree and building a semantic bundle…");
      try {
        const ingest = await api.ingestProject(path);
        setProject(ingest);
      } catch (err) {
        chat.setError(errorMessage(err));
      } finally {
        setProjectBusy(false);
        setActivity(null);
      }
    },
    [chat],
  );

  const pickProject = useCallback(async () => {
    try {
      const picked = await open({ multiple: false, directory: true });
      if (typeof picked === "string") await attachProject(picked);
    } catch (err) {
      chat.setError(errorMessage(err));
    }
  }, [attachProject, chat]);

  // A folder dropped anywhere on the window is ingested without a dialog.
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void import("@tauri-apps/api/webview")
      .then(({ getCurrentWebview }) =>
        getCurrentWebview().onDragDropEvent((event) => {
          if (event.payload.type !== "drop") return;
          const path = event.payload.paths?.[0];
          if (path) void attachProject(path);
        }),
      )
      .then((stop) => {
        if (cancelled) stop();
        else unlisten = stop;
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [attachProject]);

  const runBenchmark = useCallback(async () => {
    setBenchmarkBusy(true);
    try {
      setBenchmark(await api.benchmarkStorage(settings?.models_dir));
    } catch (err) {
      chat.setError(errorMessage(err));
    } finally {
      setBenchmarkBusy(false);
    }
  }, [chat, settings?.models_dir]);

  const applyEngineSettings = useCallback(
    async (patch: Partial<AppSettings>) => {
      if (!settings) return;
      await save({ ...settings, ...patch });
    },
    [save, settings],
  );

  const restartEngine = useCallback(async () => {
    try {
      await engine.stop();
      await engine.start();
    } catch (err) {
      chat.setError(errorMessage(err));
    }
  }, [chat, engine]);

  const resetEnginePanel = useCallback(
    async (runtime: Partial<AppSettings>) => {
      if (!settings) return;
      try {
        await save({ ...settings, ...runtime });
        setPreferences((previous) => ({
          ...DEFAULT_ENGINE_PREFERENCES,
          internet: previous.internet,
        }));
        setSystemPrompt("");
      } catch (err) {
        chat.setError(errorMessage(err));
      }
    },
    [chat, save, setPreferences, setSystemPrompt, settings],
  );

  const switchEngine = useCallback(
    async (kind: EngineKind) => {
      await engine.switchTo(kind);
    },
    [engine],
  );

  const configuredContextMax =
    activeKind === "llama"
      ? settings?.llama_ctx_size ?? 16384
      : settings?.ctx_size ?? 16384;
  // Settings can change while the old child is still running. Budget against
  // the context the active child actually launched with; the settings value
  // only becomes real after a restart.
  const contextMax = engine.active?.context_window || configuredContextMax;
  const imageTokens = images.reduce(
    (total, image) => total + image.tokens,
    0,
  );

  const handleSend = useCallback(async () => {
    const text = input.trim();
    if (!text || chat.busy || !ready) return;

    let citations: Citation[] | undefined;
    let extraContext = project?.bundle ?? "";

    if (preferences.internet) {
      setActivity(`Searching DuckDuckGo for “${text.slice(0, 64)}”…`);
      try {
        const search = await api.webSearch(text, 3);
        setActivity(
          `Ingesting ${search.results.length} page${
            search.results.length === 1 ? "" : "s"
          } · ${search.bytes > 0 ? `${(search.bytes / 1024).toFixed(0)} KB` : ""} → prefill layer…`,
        );
        if (!search.results.length) {
          setActivity(null);
          chat.setError("The web search returned no readable pages.");
          return;
        }
        citations = search.results.map((result) => ({
          title: result.title,
          url: result.url,
          snippet: result.snippet,
        }));
        extraContext = [extraContext, buildWebContext(text, search.results)]
          .filter(Boolean)
          .join("\n\n");
      } catch (err) {
        setActivity(null);
        chat.setError(errorMessage(err));
        return;
      }
    }

    const recalled = recallFacts(memory.facts, text);
    const memoryBlock = recalled.length
      ? `Facts you remember about this project. Use them when relevant, and ignore any that no longer apply:\n${recalled
          .map((fact) => `- ${fact.text}`)
          .join("\n")}`
      : "";

    setActivity(null);
    try {
      await chat.send(text, {
        systemPrompt: [CORE_SYSTEM_PROMPT, systemPrompt, memoryBlock]
          .filter(Boolean)
          .join("\n\n"),
        temperature: preferences.temperature,
        topP: preferences.topP,
        topK: preferences.topK ?? 40,
        minP: preferences.minP,
        repeatPenalty: preferences.repeatPenalty,
        thinking: preferences.thinking,
        structuredOutput: preferences.structuredOutput,
        contextPrefetch: preferences.contextPrefetch,
        maxTokens: preferences.maxTokens,
        contextWindow: contextMax,
        imageTokens,
        images: images.map((image) => image.data_url),
        extraContext,
        citations,
        mcpServers: mcpServers
          .filter((server) => server.enabled && server.command.trim())
          .map((server) => ({
            name: server.name,
            command: server.command,
            args: server.args,
            env: server.env ?? "",
            cwd: server.cwd ?? "",
          })),
        onAccepted: () => {
          setInput("");
          setImages([]);
        },
      });
    } catch (err) {
      chat.setError(errorMessage(err));
    }
  }, [
    chat,
    contextMax,
    images,
    imageTokens,
    input,
    mcpServers,
    memory.facts,
    preferences,
    project,
    ready,
    systemPrompt,
  ]);

  const messageChars = chat.messages.reduce(
    (total, message) =>
      total + message.content.length + message.reasoning.length,
    0,
  );
  const estimatedTokens =
    Math.ceil(
      (messageChars +
        systemPrompt.length +
        input.length +
        (project?.bundle.length ?? 0)) /
        4,
    ) + images.reduce((total, image) => total + image.tokens, 0);
  const contextUsed = chat.usage?.total_tokens ?? estimatedTokens;

  return (
    <div className="flex h-full min-h-0">
      <SessionsRail
        sessions={chat.sessions}
        activeId={chat.activeId}
        streamingSessionId={chat.streamingSessionId}
        collapsed={sessionsCollapsed}
        onToggle={() => setSessionsCollapsed((value) => !value)}
        onSelect={chat.setActiveId}
        onNew={chat.newChat}
        onDelete={(id) => void chat.deleteChat(id)}
        memory={memory}
      />

      <section className="flex min-w-0 flex-1 flex-col">
        <div className="flex h-12 shrink-0 items-center justify-between gap-3 border-b border-white/5 px-4">
          <div className="flex min-w-0 items-center gap-2 text-[11px] text-slate-500">
            <Cpu className="h-3.5 w-3.5 shrink-0 text-slate-600" />
            <select
              value={selectedModelPath}
              disabled={switchingModel || engine.busy}
              onChange={(event) => chooseAnyModel(event.target.value)}
              className="max-w-[260px] truncate rounded-lg border border-white/10 bg-ink-900 px-2 py-1 text-[11px] text-slate-300 outline-none transition hover:border-white/20 focus:border-acid-400/40 disabled:opacity-50"
              title={selectedModelPath || "Select a local model"}
            >
              <option value="">Select a local model…</option>
              {models.length ? (
                <optgroup label="Janas (.jns)">
                  {models.map((model) => (
                    <option key={model.path} value={model.path}>
                      {model.name}
                    </option>
                  ))}
                </optgroup>
              ) : null}
              {llamaModels.length ? (
                <optgroup label="llama.cpp (.gguf)">
                  {llamaModels.map((model) => (
                    <option key={model.path} value={model.path}>
                      {model.name}
                    </option>
                  ))}
                </optgroup>
              ) : null}
              {selectedModelPath &&
              !models.some((model) => model.path === selectedModelPath) &&
              !llamaModels.some((model) => model.path === selectedModelPath) ? (
                <option value={selectedModelPath}>
                  {basename(selectedModelPath)} (custom)
                </option>
              ) : null}
            </select>
            <span className="text-slate-700">·</span>
            <span className="shrink-0">
              {chat.messages.filter((message) => message.role === "user").length}{" "}
              turns
            </span>
            {chat.finishReason ? (
              <>
                <span className="text-slate-700">·</span>
                <span className="shrink-0 font-mono text-[10px] text-slate-600">
                  {chat.finishReason}
                </span>
              </>
            ) : null}
            {projectBusy ? (
              <span className="flex shrink-0 items-center gap-1 text-acid-300/80">
                <Loader2 className="h-3 w-3 animate-spin" />
                ingesting
              </span>
            ) : null}
          </div>
          <div className="flex shrink-0 items-center gap-1.5">
            <Button
              size="sm"
              variant="ghost"
              onClick={() => void pickProject()}
              disabled={projectBusy}
              icon={<FolderOpen className="h-3.5 w-3.5" />}
            >
              Attach project
            </Button>
            <Button
              size="sm"
              variant="ghost"
              onClick={() => setMcpOpen((value) => !value)}
              icon={<Terminal className="h-3.5 w-3.5" />}
            >
              Tools
            </Button>
            <Button
              size="sm"
              variant="ghost"
              onClick={() => void chat.clear()}
              disabled={!chat.messages.length}
              icon={<Trash2 className="h-3.5 w-3.5" />}
            >
              Clear
            </Button>
          </div>
        </div>

        <div className="min-h-0 flex-1 overflow-y-auto">
          <div className="mx-auto max-w-3xl px-6 py-6">
            {!ready ? (
              <div className="mb-5 flex flex-wrap items-center gap-3 rounded-xl border border-amber-400/20 bg-amber-400/5 px-4 py-3">
                <AlertTriangle className="h-4 w-4 shrink-0 text-amber-400" />
                <div className="min-w-0 flex-1">
                  <p className="text-xs font-medium text-amber-200">
                    The {activeKind === "llama" ? "llama.cpp" : "Janas"} engine
                    is not ready.
                  </p>
                  <p className="mt-0.5 truncate text-[10px] text-amber-200/60">
                    {engine.snapshot.detail}
                  </p>
                </div>
                <Button
                  size="sm"
                  variant="secondary"
                  busy={engine.busy}
                  onClick={() => void engine.start()}
                  icon={<Play className="h-3 w-3" />}
                >
                  Start engine
                </Button>
              </div>
            ) : null}

            {!chat.messages.length ? (
              <EmptyState
                modelName={modelName}
                ready={ready}
                onSuggestion={(prompt) => setInput(prompt)}
              />
            ) : (
              <div className="space-y-7">
                {chat.messages.map((message, index) => (
                  <MessageBoundary
                    key={message.id}
                    text={message.content}
                    resetKey={message.id}
                  >
                    <MessageBubble
                      message={message}
                      streaming={
                        chat.streaming &&
                        index === chat.messages.length - 1 &&
                        message.role === "assistant"
                      }
                      onDelete={chat.deleteMessage}
                    />
                  </MessageBoundary>
                ))}
              </div>
            )}
            <div ref={endRef} className="h-2" />
          </div>
        </div>

        <Composer
          value={input}
          onChange={setInput}
          onSend={() => void handleSend()}
          onStop={() => void chat.stop()}
          streaming={chat.busy}
          ready={ready}
          contextUsed={contextUsed}
          contextMax={contextMax}
          internet={preferences.internet}
          onInternetChange={(value) => update("internet", value)}
          images={images}
          onAddImages={setImages}
          onRemoveImage={(index) =>
            setImages((previous) => previous.filter((_, i) => i !== index))
          }
          projectName={project?.name ?? null}
          projectTokens={project?.tokens ?? 0}
          onPickProject={() => void pickProject()}
          onClearProject={() => setProject(null)}
          activity={activity}
          error={chat.error ?? modelError}
          onDismissError={() => {
            chat.setError(null);
            setModelError(null);
          }}
        />

        <TelemetryPanel
          collapsed={telemetryCollapsed}
          onToggle={() => setTelemetryCollapsed((value) => !value)}
          telemetry={telemetry.telemetry}
          history={telemetry.history}
          benchmark={benchmark}
          benchmarkBusy={benchmarkBusy}
          onBenchmark={() => void runBenchmark()}
          tokensGenerated={
            chat.usage?.completion_tokens ?? Math.round(chat.liveBytes / 4)
          }
          streaming={chat.busy}
        />
      </section>

      <EnginePanel
        collapsed={engineCollapsed}
        onToggle={() => setEngineCollapsed((value) => !value)}
        preferences={preferences}
        onUpdate={update}
        settings={settings}
        activeEngine={activeKind}
        engineTelemetry={engine.telemetry}
        engineRunning={engine.snapshot.pid !== null}
        engineBusy={engine.busy}
        engineStatus={engine.snapshot.status}
        onSwitchEngine={switchEngine}
        onApplyEngineSettings={applyEngineSettings}
        onRestartEngine={restartEngine}
        onReset={resetEnginePanel}
        systemPrompt={systemPrompt}
        onSystemPromptChange={setSystemPrompt}
        usage={chat.usage}
        elapsedMs={chat.elapsedMs}
        firstTokenMs={chat.firstTokenMs}
        liveBytes={chat.liveBytes}
        streaming={chat.streaming}
        startedAtMs={chat.startedAtMs}
      />

      <McpLogDrawer open={mcpOpen} onClose={() => setMcpOpen(false)} />
    </div>
  );
}

function EmptyState({
  modelName,
  ready,
  onSuggestion,
}: {
  modelName: string;
  ready: boolean;
  onSuggestion: (prompt: string) => void;
}) {
  return (
    <div className="flex flex-col items-center py-10 text-center">
      <div className="relative">
        <div className="absolute inset-0 -z-10 rounded-full bg-acid-400/10 blur-2xl" />
        <Logo className="h-16 w-16" />
      </div>
      <h2 className="mt-6 text-xl font-semibold tracking-tight text-white">
        Your own inference bench
      </h2>
      <p className="mt-2 max-w-md text-[12.5px] leading-6 text-slate-500">
        {ready
          ? `${modelName} is loaded and listening. Every panel collapses; the centre stays pure text.`
          : "Select a model above to start its engine. Everything runs locally through the selected engine."}
      </p>

      <div className="mt-8 grid w-full max-w-2xl gap-2 sm:grid-cols-3">
        {SUGGESTIONS.map((suggestion) => (
          <button
            key={suggestion.title}
            type="button"
            onClick={() => onSuggestion(suggestion.prompt)}
            disabled={!ready}
            className="group rounded-xl border border-white/10 bg-white/[0.02] p-3.5 text-left transition hover:border-acid-400/25 hover:bg-white/[0.04] disabled:cursor-not-allowed disabled:opacity-50"
          >
            <p className="text-[11px] font-medium text-slate-300 group-hover:text-white">
              {suggestion.title}
            </p>
            <p className="mt-1 line-clamp-3 text-[10px] leading-4 text-slate-600">
              {suggestion.prompt}
            </p>
          </button>
        ))}
      </div>

      <div className="mt-8 flex flex-wrap items-center justify-center gap-2 text-[10px] text-slate-600">
        {[
          "Prompt / reply ticker",
          "Context ring",
          "Web + citations",
          "Local memory",
          "SSD telemetry",
          "Expert heatmap",
        ].map((chip) => (
          <span
            key={chip}
            className="rounded-full border border-white/5 bg-white/[0.02] px-2.5 py-1"
          >
            {chip}
          </span>
        ))}
      </div>
    </div>
  );
}
