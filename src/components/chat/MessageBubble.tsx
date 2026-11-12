import { useState } from "react";
import { Check, Copy, ExternalLink, Trash2 } from "lucide-react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

import { api } from "../../lib/api";
import { errorMessage } from "../../lib/format";
import type { ChatMessage, Citation } from "../../types";
import { Logo } from "../Logo";

interface MessageBubbleProps {
  message: ChatMessage;
  streaming: boolean;
  onDelete?: (id: string) => void;
}

function safeCitations(citations: Citation[] | undefined): Citation[] {
  if (!Array.isArray(citations)) return [];
  return citations.filter((citation) => {
    if (!citation || typeof citation.url !== "string") return false;
    try {
      const url = new URL(citation.url);
      return url.protocol === "http:" || url.protocol === "https:";
    } catch {
      return false;
    }
  });
}

function citationLabel(citation: Citation, index: number): string {
  const title = typeof citation.title === "string" ? citation.title.trim() : "";
  if (title && title.length > 2 && title.toLowerCase() !== "go") return title;
  try {
    return new URL(citation.url).hostname.replace(/^www\./, "");
  } catch {
    return `Source ${index + 1}`;
  }
}

function isWebUrl(href: string | undefined): href is string {
  return Boolean(href && /^https?:\/\//i.test(href));
}

export function MessageBubble({
  message,
  streaming,
  onDelete,
}: MessageBubbleProps) {
  const [copied, setCopied] = useState(false);
  const [linkError, setLinkError] = useState<string | null>(null);
  const isUser = message.role === "user";
  const content = typeof message.content === "string" ? message.content : "";
  const reasoning =
    typeof message.reasoning === "string" ? message.reasoning : "";
  const citations = safeCitations(message.citations);
  const empty = !content && !reasoning;

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(content);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1_600);
    } catch {
      // Clipboard permissions may be unavailable; the text stays selectable.
    }
  };

  const openLink = async (url: string) => {
    setLinkError(null);
    try {
      await api.openExternal(url);
    } catch (err) {
      setLinkError(errorMessage(err));
    }
  };

  const actions = (
    <div className="flex items-center gap-0.5 opacity-0 transition group-hover:opacity-100 focus-within:opacity-100">
      <button
        type="button"
        onClick={() => void copy()}
        className="inline-flex items-center gap-1 rounded-md px-1.5 py-1 text-[10px] text-slate-600 transition hover:bg-white/5 hover:text-slate-300"
        title="Copy message"
      >
        {copied ? (
          <Check className="h-3 w-3 text-emerald-400" />
        ) : (
          <Copy className="h-3 w-3" />
        )}
        {copied ? "Copied" : "Copy"}
      </button>
      {onDelete ? (
        <button
          type="button"
          onClick={() => onDelete(message.id)}
          className="rounded-md px-1.5 py-1 text-slate-600 transition hover:bg-rose-500/10 hover:text-rose-300"
          title="Delete message"
          aria-label="Delete message"
        >
          <Trash2 className="h-3 w-3" />
        </button>
      ) : null}
    </div>
  );

  if (isUser) {
    return (
      <div className="group flex animate-fade-in flex-col items-end gap-1">
        <div className="max-w-[82%] rounded-2xl rounded-br-md border border-acid-400/20 bg-acid-400/10 px-4 py-3">
          {message.images?.length ? (
            <div className="mb-2 flex flex-wrap gap-1.5">
              {message.images.map((image, index) => (
                <img
                  key={`${image.name}-${index}`}
                  src={image.data_url}
                  alt={image.name}
                  className="h-20 w-20 rounded-lg border border-white/10 object-cover"
                />
              ))}
            </div>
          ) : null}
          <p className="whitespace-pre-wrap text-[13.5px] leading-6 text-slate-100">
            {content}
          </p>
          <p className="mt-1.5 text-right text-[10px] text-acid-200/40">
            {timeLabel(message.created_at)}
          </p>
        </div>
        {actions}
      </div>
    );
  }

  return (
    <div className="group flex animate-fade-in gap-3">
      <Logo className="mt-0.5 h-7 w-7 shrink-0" />
      <div className="min-w-0 flex-1">
        <div className="mb-1.5 flex items-center gap-2">
          <span className="text-[11px] font-semibold text-slate-300">Janas</span>
          <span className="text-[10px] text-slate-600">
            {timeLabel(message.created_at)}
          </span>
          {streaming ? (
            <span className="flex items-center gap-1.5 text-[10px] text-acid-300">
              <span className="h-1 w-1 animate-pulse-soft rounded-full bg-acid-400" />
              generating
            </span>
          ) : null}
        </div>

        {reasoning ? (
          <details className="mb-3 rounded-xl border border-white/5 bg-white/[0.02] px-3 py-2">
            <summary className="cursor-pointer select-none text-[10px] font-semibold uppercase tracking-[0.14em] text-slate-500 transition hover:text-slate-400">
              Reasoning
            </summary>
            <div className="mt-2 max-h-64 overflow-y-auto whitespace-pre-wrap text-[11.5px] leading-6 text-slate-500">
              {reasoning}
            </div>
          </details>
        ) : null}

        {empty && streaming ? (
          <div className="flex items-center gap-1 py-2">
            <span className="h-1.5 w-1.5 animate-pulse-soft rounded-full bg-acid-400" />
            <span className="h-1.5 w-1.5 animate-pulse-soft rounded-full bg-acid-400 [animation-delay:150ms]" />
            <span className="h-1.5 w-1.5 animate-pulse-soft rounded-full bg-acid-400 [animation-delay:300ms]" />
          </div>
        ) : (
          <div className="md">
            <ReactMarkdown
              remarkPlugins={[remarkGfm]}
              components={{
                // The webview cannot navigate away from the studio, and the
                // system opener keeps http(s) links in the user's browser.
                a: ({ href, children }) => (
                  <a
                    href={isWebUrl(href) ? href : undefined}
                    onClick={(event) => {
                      if (!isWebUrl(href)) return;
                      event.preventDefault();
                      void openLink(href);
                    }}
                    rel="noreferrer noopener"
                  >
                    {children}
                  </a>
                ),
              }}
            >
              {content}
            </ReactMarkdown>
            {streaming ? (
              <span className="ml-0.5 inline-block h-4 w-1.5 translate-y-0.5 animate-pulse-soft rounded-sm bg-acid-400/80" />
            ) : null}
          </div>
        )}

        {citations.length ? (
          <div className="mt-3 flex flex-wrap items-center gap-1.5">
            <span className="text-[9px] uppercase tracking-wider text-slate-600">
              Sources
            </span>
            {citations.map((citation, index) => {
              const label = citationLabel(citation, index);
              return (
                <a
                  key={`${citation.url}-${index}`}
                  href={citation.url}
                  onClick={(event) => {
                    event.preventDefault();
                    void openLink(citation.url);
                  }}
                  title={citation.snippet || citation.url}
                  aria-label={`Open source ${index + 1}: ${label}`}
                  className="inline-flex max-w-[260px] items-center gap-1 rounded-full border border-sky-400/20 bg-sky-400/5 px-2 py-0.5 text-[9.5px] text-sky-200/90 transition hover:border-sky-400/40 hover:bg-sky-400/10"
                >
                  <span className="font-mono text-[8px] text-sky-400/70">
                    {index + 1}
                  </span>
                  <span className="truncate">{label}</span>
                  <ExternalLink className="h-2.5 w-2.5 shrink-0 opacity-60" />
                </a>
              );
            })}
          </div>
        ) : null}

        {linkError ? (
          <p className="mt-1.5 text-[10px] text-rose-300">{linkError}</p>
        ) : null}

        {!streaming ? <div className="mt-2 flex items-center gap-1">{actions}</div> : null}
      </div>
    </div>
  );
}

function timeLabel(timestamp: number): string {
  return new Date(timestamp).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
  });
}
