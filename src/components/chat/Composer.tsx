import { useEffect, useRef, useState } from "react";
import {
  ArrowUp,
  FolderOpen,
  Globe,
  ImagePlus,
  Loader2,
  Square,
  X,
} from "lucide-react";

import { cn } from "../../lib/cn";
import { errorMessage, formatCount } from "../../lib/format";
import { Button } from "../ui/Button";
import { ProgressBar } from "../ui/ProgressBar";

export interface ComposerImage {
  name: string;
  data_url: string;
  tokens: number;
}

interface ComposerProps {
  value: string;
  onChange: (value: string) => void;
  onSend: () => void;
  onStop: () => void;
  streaming: boolean;
  ready: boolean;
  contextUsed: number;
  contextMax: number;
  internet: boolean;
  onInternetChange: (value: boolean) => void;
  images: ComposerImage[];
  onAddImages: (images: ComposerImage[]) => void;
  onRemoveImage: (index: number) => void;
  projectName: string | null;
  projectTokens: number;
  onPickProject: () => void;
  onClearProject: () => void;
  activity: string | null;
  error: string | null;
  onDismissError: () => void;
}

function readImage(file: File): Promise<ComposerImage> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(new Error("Could not read that image."));
    reader.onload = () => {
      const dataUrl = String(reader.result);
      const image = new Image();
      image.onload = () => {
        const tokens =
          Math.ceil(image.width / 28) * Math.ceil(image.height / 28) + 1;
        resolve({ name: file.name, data_url: dataUrl, tokens });
      };
      image.onerror = () => reject(new Error("That image could not be decoded."));
      image.src = dataUrl;
    };
    reader.readAsDataURL(file);
  });
}

export function Composer({
  value,
  onChange,
  onSend,
  onStop,
  streaming,
  ready,
  contextUsed,
  contextMax,
  internet,
  onInternetChange,
  images,
  onAddImages,
  onRemoveImage,
  projectName,
  projectTokens,
  onPickProject,
  onClearProject,
  activity,
  error,
  onDismissError,
}: ComposerProps) {
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileRef = useRef<HTMLInputElement>(null);

  const [localError, setLocalError] = useState<string | null>(null);
  const [dragging, setDragging] = useState(false);

  useEffect(() => {
    const element = textareaRef.current;
    if (!element) return;
    element.style.height = "0px";
    element.style.height = `${Math.min(element.scrollHeight, 220)}px`;
  }, [value]);

  const addFiles = async (files: FileList | File[]) => {
    const picked = Array.from(files)
      .filter((file) => file.type.startsWith("image/"))
      .slice(0, 4 - images.length);
    if (!picked.length) return;
    setLocalError(null);
    try {
      const parsed = await Promise.all(picked.map(readImage));
      onAddImages([...images, ...parsed]);
    } catch (err) {
      setLocalError(errorMessage(err));
    }
  };

  const contextPercent =
    contextMax > 0 ? Math.min(1, contextUsed / contextMax) : 0;
  const ringColor =
    contextPercent > 0.9
      ? "#fb7185"
      : contextPercent > 0.7
        ? "#fbbf24"
        : "#cdf75a";

  return (
    <div className="shrink-0 border-t border-white/5 bg-ink-950/80 px-6 py-3.5">
      <div className="mx-auto max-w-3xl">
        {error ? (
          <div className="mb-2 flex items-start gap-2 rounded-xl border border-rose-400/20 bg-rose-500/5 px-3.5 py-2.5 text-[11px] leading-5 text-rose-300">
            <span className="flex-1">{error}</span>
            <button
              type="button"
              onClick={onDismissError}
              className="text-rose-400/60 transition hover:text-rose-200"
              aria-label="Dismiss error"
            >
              <X className="h-3.5 w-3.5" />
            </button>
          </div>
        ) : null}

        {localError ? (
          <div className="mb-2 flex items-start gap-2 rounded-xl border border-amber-400/20 bg-amber-400/5 px-3.5 py-2.5 text-[11px] leading-5 text-amber-200">
            <span className="flex-1">{localError}</span>
            <button
              type="button"
              onClick={() => setLocalError(null)}
              className="text-amber-300/60 transition hover:text-amber-100"
              aria-label="Dismiss message"
            >
              <X className="h-3.5 w-3.5" />
            </button>
          </div>
        ) : null}

        {activity ? (
          <div className="mb-2 rounded-xl border border-sky-400/20 bg-sky-400/5 px-3 py-2">
            <div className="flex items-center gap-2 text-[10.5px] text-sky-200">
              <Loader2 className="h-3 w-3 animate-spin" />
              <span className="min-w-0 flex-1 truncate">{activity}</span>
              <span className="text-[9px] uppercase tracking-wider text-sky-400/60">
                web pipeline
              </span>
            </div>
            <ProgressBar value={null} tone="sky" className="mt-1.5" />
          </div>
        ) : null}

        {(projectName || images.length) ? (
          <div className="mb-2 flex flex-wrap items-center gap-1.5">
            {projectName ? (
              <span className="inline-flex items-center gap-1.5 rounded-full border border-acid-400/20 bg-acid-400/5 px-2.5 py-1 text-[10px] text-acid-200/90">
                <FolderOpen className="h-3 w-3" />
                {projectName}
                <span className="font-mono text-[9px] text-acid-500/70">
                  {formatCount(projectTokens)} tok
                </span>
                <button
                  type="button"
                  onClick={onClearProject}
                  className="text-acid-400/60 hover:text-acid-100"
                  aria-label="Remove project context"
                >
                  <X className="h-3 w-3" />
                </button>
              </span>
            ) : null}
            {images.map((image, index) => (
              <span
                key={`${image.name}-${index}`}
                className="inline-flex items-center gap-1.5 rounded-full border border-white/10 bg-white/5 py-1 pl-1 pr-2 text-[10px] text-slate-300"
              >
                <img
                  src={image.data_url}
                  alt={image.name}
                  className="h-5 w-5 rounded object-cover"
                />
                {image.name.slice(0, 16)}
                <span className="font-mono text-[9px] text-slate-600">
                  {image.tokens} tok
                </span>
                <button
                  type="button"
                  onClick={() => onRemoveImage(index)}
                  className="text-slate-500 hover:text-slate-200"
                  aria-label="Remove image"
                >
                  <X className="h-3 w-3" />
                </button>
              </span>
            ))}
          </div>
        ) : null}

        <div
          onDragOver={(event) => {
            event.preventDefault();
            setDragging(true);
          }}
          onDragLeave={() => setDragging(false)}
          onDrop={(event) => {
            event.preventDefault();
            setDragging(false);
            void addFiles(event.dataTransfer.files);
          }}
          className={cn(
            "rounded-2xl border bg-ink-900 p-2 shadow-panel transition",
            dragging
              ? "border-acid-400/50 bg-acid-400/[0.04]"
              : "border-white/10 focus-within:border-acid-400/30",
          )}
        >
          <textarea
            ref={textareaRef}
            rows={1}
            value={value}
            disabled={!ready || streaming}
            onChange={(event) => onChange(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter" && !event.shiftKey) {
                event.preventDefault();
                if (ready && !streaming) onSend();
              }
            }}
            placeholder={
              ready
                ? internet
                  ? "Ask the model, and it will read the web first…"
                  : "Message your local model…"
                : "Start the Janas engine to begin chatting"
            }
            className="max-h-[220px] min-h-[52px] w-full resize-none bg-transparent px-3 py-2.5 text-[13.5px] leading-6 text-slate-100 outline-none placeholder:text-slate-600 disabled:cursor-not-allowed"
          />

          <div className="flex items-center justify-between gap-2 px-1 pb-0.5">
            <div className="flex items-center gap-1">
              <input
                ref={fileRef}
                type="file"
                accept="image/*"
                multiple
                className="hidden"
                onChange={(event) => {
                  if (event.target.files) void addFiles(event.target.files);
                  event.target.value = "";
                }}
              />
              <IconButton
                label="Attach images"
                onClick={() => fileRef.current?.click()}
                disabled={images.length >= 4}
              >
                <ImagePlus className="h-3.5 w-3.5" />
              </IconButton>
              <IconButton
                label="Attach a local project folder"
                onClick={onPickProject}
                disabled={!ready}
              >
                <FolderOpen className="h-3.5 w-3.5" />
              </IconButton>
              <button
                type="button"
                onClick={() => onInternetChange(!internet)}
                className={cn(
                  "inline-flex h-7 items-center gap-1.5 rounded-lg border px-2 text-[10px] transition",
                  internet
                    ? "border-sky-400/30 bg-sky-400/10 text-sky-200"
                    : "border-white/10 bg-white/5 text-slate-500 hover:text-slate-300",
                )}
                aria-pressed={internet}
                title="Search the web and bundle the top pages into the prompt"
              >
                <Globe className="h-3 w-3" />
                {internet ? "Web on" : "Web off"}
              </button>

            </div>

            <div className="flex items-center gap-2">
              <ContextRing
                used={contextUsed}
                max={contextMax}
                percent={contextPercent}
                color={ringColor}
              />

              {streaming ? (
                <Button
                  size="sm"
                  variant="danger"
                  onClick={onStop}
                  icon={<Square className="h-3 w-3" />}
                >
                  Stop
                </Button>
              ) : (
                <Button
                  size="sm"
                  variant="primary"
                  disabled={!ready || !value.trim()}
                  onClick={onSend}
                  icon={<ArrowUp className="h-3.5 w-3.5" />}
                >
                  Send
                </Button>
              )}
            </div>
          </div>
        </div>

        <div className="mt-1.5 flex items-center justify-between text-[9.5px] text-slate-600">
          <span>
            Enter to send · Shift + Enter for a new line ·{" "}
            <span className="text-acid-500/80">
              {ready ? "local engine" : "engine offline"}
            </span>
          </span>
          <span className="font-mono">
            {formatCount(contextUsed)} / {formatCount(contextMax)} ctx
          </span>
        </div>
      </div>
    </div>
  );
}

function IconButton({
  children,
  label,
  onClick,
  disabled,
}: {
  children: React.ReactNode;
  label: string;
  onClick: () => void;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      title={label}
      aria-label={label}
      className="inline-flex h-7 w-7 items-center justify-center rounded-lg border border-white/10 bg-white/5 text-slate-500 transition hover:text-slate-200 disabled:cursor-not-allowed disabled:opacity-40"
    >
      {children}
    </button>
  );
}

function ContextRing({
  used,
  max,
  percent,
  color,
}: {
  used: number;
  max: number;
  percent: number;
  color: string;
}) {
  const radius = 9;
  const circumference = 2 * Math.PI * radius;
  return (
    <div
      className="relative flex h-7 w-7 items-center justify-center"
      title={`Context ${formatCount(used)} of ${formatCount(max)} tokens (${Math.round(
        percent * 100,
      )}%)`}
    >
      <svg viewBox="0 0 24 24" className="h-7 w-7 -rotate-90">
        <circle
          cx="12"
          cy="12"
          r={radius}
          fill="none"
          stroke="rgba(255,255,255,0.08)"
          strokeWidth="3"
        />
        <circle
          cx="12"
          cy="12"
          r={radius}
          fill="none"
          stroke={color}
          strokeWidth="3"
          strokeLinecap="round"
          strokeDasharray={circumference}
          strokeDashoffset={circumference * (1 - percent)}
          className="transition-all duration-500"
        />
      </svg>
      <span className="absolute font-mono text-[7px] text-slate-500">
        {Math.round(percent * 100)}
      </span>
    </div>
  );
}
