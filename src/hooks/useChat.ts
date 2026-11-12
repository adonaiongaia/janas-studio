import { useCallback, useEffect, useRef, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import { api, Events } from "../lib/api";
import { fitContext } from "../lib/contextBudget";
import { errorMessage, uid } from "../lib/format";
import { appendMcpLog } from "../lib/mcpLog";
import type {
  ChatDelta,
  ChatDone,
  ChatErrorEvent,
  ChatMessage,
  ChatNotice,
  ChatSession,
  ChatUsage,
  Citation,
  McpActivityEvent,
  McpServerInput,
} from "../types";

const STORAGE_KEY = "janas.sessions.v1";
const ACTIVE_KEY = "janas.activeSession.v1";

export interface SendOptions {
  systemPrompt: string;
  temperature: number;
  topP: number;
  topK: number;
  minP: number;
  repeatPenalty: number;
  thinking: boolean;
  structuredOutput: boolean;
  contextPrefetch: number;
  maxTokens: number | null;
  contextWindow: number;
  imageTokens?: number;
  images?: string[];
  extraContext?: string;
  citations?: Citation[];
  mcpServers?: McpServerInput[];
  /** Called once the token budget accepts the turn and adds it to the session. */
  onAccepted?: () => void;
}

function createSession(): ChatSession {
  const now = Date.now();
  return {
    id: uid(),
    title: "New chat",
    created_at: now,
    updated_at: now,
    messages: [],
  };
}

function normalizeCitation(value: unknown): Citation | null {
  if (!value || typeof value !== "object") return null;
  const citation = value as Partial<Citation>;
  if (
    typeof citation.url !== "string" ||
    !/^https?:\/\//i.test(citation.url)
  ) {
    return null;
  }
  return {
    title:
      typeof citation.title === "string" && citation.title.trim()
        ? citation.title
        : citation.url,
    url: citation.url,
    snippet: typeof citation.snippet === "string" ? citation.snippet : undefined,
  };
}

function normalizeMessage(value: unknown): ChatMessage | null {
  if (!value || typeof value !== "object") return null;
  const message = value as Partial<ChatMessage>;
  if (message.role !== "user" && message.role !== "assistant") return null;
  const images = Array.isArray(message.images)
    ? message.images
        .filter(
          (image): image is { name: string; data_url: string; tokens: number } =>
            Boolean(image) &&
            typeof image.name === "string" &&
            typeof image.data_url === "string",
        )
        .map((image) => ({
          ...image,
          tokens: typeof image.tokens === "number" ? image.tokens : 0,
        }))
    : undefined;
  return {
    id: typeof message.id === "string" ? message.id : uid(),
    role: message.role,
    content: typeof message.content === "string" ? message.content : "",
    reasoning: typeof message.reasoning === "string" ? message.reasoning : "",
    created_at:
      typeof message.created_at === "number" ? message.created_at : Date.now(),
    citations: Array.isArray(message.citations)
      ? message.citations
          .map(normalizeCitation)
          .filter((citation): citation is Citation => citation !== null)
      : undefined,
    images,
  };
}

function loadSessions(): ChatSession[] {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (raw) {
      const parsed: unknown = JSON.parse(raw);
      if (Array.isArray(parsed) && parsed.length) {
        const sessions = parsed
          .filter(
            (session): session is ChatSession =>
              Boolean(session) &&
              typeof session === "object" &&
              typeof (session as ChatSession).id === "string" &&
              Array.isArray((session as ChatSession).messages),
          )
          .map((session) => ({
            ...session,
            title:
              typeof session.title === "string" ? session.title : "New chat",
            created_at:
              typeof session.created_at === "number"
                ? session.created_at
                : Date.now(),
            updated_at:
              typeof session.updated_at === "number"
                ? session.updated_at
                : Date.now(),
            messages: session.messages
              .map(normalizeMessage)
              .filter((message): message is ChatMessage => message !== null),
          }));
        if (sessions.length) return sessions;
      }
    }
  } catch {
    // A corrupt store should never stop the studio from opening.
  }
  return [createSession()];
}

function loadActive(sessions: ChatSession[]): string {
  try {
    const stored = window.localStorage.getItem(ACTIVE_KEY);
    if (stored) {
      const id = JSON.parse(stored) as string;
      if (sessions.some((session) => session.id === id)) return id;
    }
  } catch {
    // fall through to the first session
  }
  return sessions[0]?.id ?? "";
}

export function useChat() {
  const [sessions, setSessions] = useState<ChatSession[]>(loadSessions);
  // Resolve the active chat against the sessions the state actually holds.
  // Calling loadSessions() again here would mint a second, unrelated session
  // on a fresh profile, leaving activeId pointing at a chat that is not in
  // the list. Sending then had no session to append to and returned silently.
  const [activeId, setActiveId] = useState<string>(() =>
    loadActive(sessions),
  );
  const [streaming, setStreaming] = useState(false);
  const [streamingSessionId, setStreamingSessionId] = useState<string | null>(
    null,
  );
  const [error, setError] = useState<string | null>(null);
  const [usage, setUsage] = useState<ChatUsage | null>(null);
  const [elapsedMs, setElapsedMs] = useState<number | null>(null);
  const [firstTokenMs, setFirstTokenMs] = useState<number | null>(null);
  const [finishReason, setFinishReason] = useState<string | null>(null);
  const [liveBytes, setLiveBytes] = useState(0);
  const [startedAtMs, setStartedAtMs] = useState<number | null>(null);
  const activeRequest = useRef<{ requestId: string; sessionId: string } | null>(
    null,
  );
  const streamStartedAt = useRef<number | null>(null);
  const sessionsRef = useRef(sessions);
  sessionsRef.current = sessions;
  const activeIdRef = useRef(activeId);
  activeIdRef.current = activeId;

  useEffect(() => {
    try {
      // Keep the newest 60 sessions; messages are plain text.
      window.localStorage.setItem(
        STORAGE_KEY,
        JSON.stringify(sessions.slice(0, 60)),
      );
    } catch {
      // Storage can be full or disabled. The session still works in memory.
    }
  }, [sessions]);

  useEffect(() => {
    try {
      window.localStorage.setItem(ACTIVE_KEY, JSON.stringify(activeId));
    } catch {
      // ignore
    }
  }, [activeId]);

  useEffect(() => {
    let disposed = false;
    const unlisteners: UnlistenFn[] = [];

    const updateMessage = (
      requestId: string,
      sessionId: string,
      updater: (message: ChatMessage) => ChatMessage,
    ) => {
      setSessions((previous) =>
        previous.map((session) =>
          session.id === sessionId
            ? {
                ...session,
                updated_at: Date.now(),
                messages: session.messages.map((message) =>
                  message.id === requestId ? updater(message) : message,
                ),
              }
            : session,
        ),
      );
    };

    const attach = async () => {
      const deltaUnlisten = await listen<ChatDelta>(
        Events.chatDelta,
        (event) => {
          const request = activeRequest.current;
          if (!request || event.payload.request_id !== request.requestId) return;
          const delta =
            typeof event.payload.delta === "string" ? event.payload.delta : "";
          const reasoning =
            typeof event.payload.reasoning === "string"
              ? event.payload.reasoning
              : "";
          if (!delta && !reasoning) return;
          // A context/tool notice is not a failed turn. Clear it as soon as
          // the model produces output so a successful reply does not leave a
          // red error banner behind.
          setError(null);
          setLiveBytes((bytes) => bytes + delta.length);
          updateMessage(request.requestId, request.sessionId, (message) => ({
            ...message,
            content: (message.content ?? "") + delta,
            reasoning: (message.reasoning ?? "") + reasoning,
          }));
        },
      );
      const doneUnlisten = await listen<ChatDone>(Events.chatDone, (event) => {
        const request = activeRequest.current;
        if (!request || event.payload.request_id !== request.requestId) return;
        setStreaming(false);
        setStreamingSessionId(null);
        setUsage(event.payload.usage);
        setElapsedMs(event.payload.elapsed_ms);
        setFirstTokenMs(event.payload.first_token_ms);
        setFinishReason(event.payload.finish_reason);
        setError(null);
        activeRequest.current = null;
      });
      const errorUnlisten = await listen<ChatErrorEvent>(
        Events.chatError,
        (event) => {
          const request = activeRequest.current;
          if (!request || event.payload.request_id !== request.requestId) return;
          setError(event.payload.message);
          setStreaming(false);
          setStreamingSessionId(null);
          activeRequest.current = null;
          updateMessage(request.requestId, request.sessionId, (message) =>
            message.content
              ? message
              : {
                  ...message,
                  content: `_${event.payload.message}_`,
                },
          );
        },
      );
      const noticeUnlisten = await listen<ChatNotice>(
        Events.chatNotice,
        (event) => {
          const request = activeRequest.current;
          if (!request || event.payload.request_id !== request.requestId) return;
          setError(event.payload.message);
        },
      );
      const mcpActivityUnlisten = await listen<McpActivityEvent>(
        Events.mcpActivity,
        (event) => {
          appendMcpLog({
            server: event.payload.server || "MCP",
            kind: event.payload.level === "error" ? "error" : "tool",
            message: event.payload.message,
          });
        },
      );

      if (disposed) {
        deltaUnlisten();
        doneUnlisten();
        errorUnlisten();
        noticeUnlisten();
        mcpActivityUnlisten();
      } else {
        unlisteners.push(
          deltaUnlisten,
          doneUnlisten,
          errorUnlisten,
          noticeUnlisten,
          mcpActivityUnlisten,
        );
      }
    };

    void attach().catch((err) => {
      setError(`Chat events could not be connected: ${errorMessage(err)}`);
    });
    return () => {
      disposed = true;
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, []);

  const send = useCallback(
    async (text: string, options: SendOptions) => {
      const trimmed = text.trim();
      if (!trimmed || activeRequest.current) return false;

      const requestedSessionId = activeIdRef.current;
      // Fall back to the first chat if the stored active id is stale, so a
      // message is never dropped without feedback.
      const session =
        sessionsRef.current.find(
          (candidate) => candidate.id === requestedSessionId,
        ) ?? sessionsRef.current[0];
      if (!session) return false;
      const sessionId = session.id;
      if (sessionId !== requestedSessionId) setActiveId(sessionId);

      const requestId = uid();
      const now = Date.now();
      const userMessage: ChatMessage = {
        id: uid(),
        role: "user",
        content: trimmed,
        reasoning: "",
        created_at: now,
        images: options.images?.length
          ? options.images.map((dataUrl, index) => ({
              name: `image-${index + 1}`,
              data_url: dataUrl,
              tokens: 0,
            }))
          : undefined,
      };
      const assistantMessage: ChatMessage = {
        id: requestId,
        role: "assistant",
        content: "",
        reasoning: "",
        created_at: now,
        citations: options.citations,
      };

      const history = [...session.messages, userMessage];
      const turnWindow = Math.max(1, options.contextPrefetch) * 2 + 1;
      const windowed = history.slice(-turnWindow).filter(
        (message) =>
          message.role === "user" ||
          message.content.trim().length > 0 ||
          message.reasoning.trim().length > 0,
      );

      const fitted = fitContext({
        contextWindow: options.contextWindow,
        maxTokens: options.maxTokens,
        systemPrompt: options.systemPrompt,
        extraContext: options.extraContext,
        messages: windowed.map((message) => ({
          role: message.role,
          content: message.content,
        })),
        imageTokens: options.imageTokens ?? 0,
      });
      if (fitted.error) {
        setError(fitted.error);
        return false;
      }

      setSessions((previous) =>
        previous.map((candidate) =>
          candidate.id === sessionId
            ? {
                ...candidate,
                title:
                  candidate.messages.length === 0
                    ? trimmed.slice(0, 56)
                    : candidate.title,
                updated_at: now,
                messages: [...candidate.messages, userMessage, assistantMessage],
              }
            : candidate,
        ),
      );
      options.onAccepted?.();
      setStreaming(true);
      setStreamingSessionId(sessionId);
      setError(null);
      setUsage(null);
      setElapsedMs(null);
      setFirstTokenMs(null);
      setFinishReason(null);
      setLiveBytes(0);
      setStartedAtMs(now);
      activeRequest.current = { requestId, sessionId };
      streamStartedAt.current = Date.now();

      try {
        await api.streamChat({
          request_id: requestId,
          messages: fitted.messages,
          system_prompt: fitted.systemPrompt || null,
          temperature: options.temperature,
          top_p: options.topP,
          top_k: Math.round(Math.min(100, Math.max(1, options.topK))),
          min_p: options.minP,
          repeat_penalty: options.repeatPenalty,
          structured: options.structuredOutput,
          thinking: options.thinking,
          images: options.images ?? [],
          max_tokens: options.maxTokens,
          mcp_servers: options.mcpServers ?? [],
        });
      } catch (err) {
        setError(errorMessage(err));
        setStreaming(false);
        setStreamingSessionId(null);
        activeRequest.current = null;
        setSessions((previous) =>
          previous.map((candidate) =>
            candidate.id === sessionId
              ? {
                  ...candidate,
                  messages: candidate.messages.map((message) =>
                    message.id === requestId && !message.content
                      ? {
                          ...message,
                          content: "_The engine did not return a response._",
                        }
                      : message,
                  ),
                }
              : candidate,
          ),
        );
      }
      return true;
    },
    [],
  );

  const stop = useCallback(async () => {
    const request = activeRequest.current;
    if (!request) return;
    try {
      await api.cancelChat(request.requestId);
    } catch (err) {
      setError(errorMessage(err));
    }
  }, []);

  const clear = useCallback(async () => {
    if (activeRequest.current) {
      try {
        await api.cancelChat(activeRequest.current.requestId);
      } catch {
        // The request may have completed between the click and the cancel.
      }
    }
    activeRequest.current = null;
    setSessions((previous) =>
      previous.map((session) =>
        session.id === activeIdRef.current
          ? { ...session, messages: [], title: "New chat" }
          : session,
      ),
    );
    setStreaming(false);
    setStreamingSessionId(null);
    setUsage(null);
    setElapsedMs(null);
    setFirstTokenMs(null);
    setFinishReason(null);
    setError(null);
    setLiveBytes(0);
  }, []);

  const newChat = useCallback(() => {
    const session = createSession();
    setSessions((previous) => [session, ...previous].slice(0, 60));
    setActiveId(session.id);
    setUsage(null);
    setElapsedMs(null);
    setFirstTokenMs(null);
    setFinishReason(null);
    setError(null);
    return session.id;
  }, []);

  const deleteChat = useCallback(async (id: string) => {
    if (activeRequest.current?.sessionId === id) {
      try {
        await api.cancelChat(activeRequest.current.requestId);
      } catch {
        // ignore
      }
      activeRequest.current = null;
      setStreaming(false);
      setStreamingSessionId(null);
    }
    setSessions((previous) => {
      const remaining = previous.filter((session) => session.id !== id);
      const next = remaining.length ? remaining : [createSession()];
      setActiveId((current) =>
        current === id ? next[0].id : current,
      );
      return next;
    });
  }, []);

  const deleteMessage = useCallback((messageId: string) => {
    setSessions((previous) =>
      previous.map((session) =>
        session.id === activeIdRef.current
          ? {
              ...session,
              messages: session.messages.filter(
                (message) => message.id !== messageId,
              ),
              updated_at: Date.now(),
            }
          : session,
      ),
    );
  }, []);

  const activeSession =
    sessions.find((session) => session.id === activeId) ?? sessions[0];
  const messages = activeSession?.messages ?? [];
  const activeStreaming = streaming && streamingSessionId === activeId;

  return {
    sessions,
    activeId,
    messages,
    streaming: activeStreaming,
    busy: streaming,
    startedAtMs,
    streamingSessionId,
    error,
    usage,
    elapsedMs,
    firstTokenMs,
    finishReason,
    liveBytes,
    streamStartedAt,
    setActiveId,
    send,
    stop,
    clear,
    newChat,
    deleteChat,
    deleteMessage,
    setError,
  };
}

export type ChatController = ReturnType<typeof useChat>;
