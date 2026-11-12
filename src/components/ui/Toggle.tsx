interface ToggleProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: string;
  description?: string;
  disabled?: boolean;
}

export function Toggle({
  checked,
  onChange,
  label,
  description,
  disabled = false,
}: ToggleProps) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className="flex w-full items-start justify-between gap-3 text-left disabled:opacity-40"
    >
      <span className="min-w-0">
        <span className="block text-xs font-medium text-slate-300">{label}</span>
        {description ? (
          <span className="mt-0.5 block text-[10px] leading-4 text-slate-600">
            {description}
          </span>
        ) : null}
      </span>
      <span
        className={`relative mt-0.5 h-4 w-7 shrink-0 rounded-full border transition ${
          checked
            ? "border-acid-400/40 bg-acid-400/80"
            : "border-white/10 bg-white/10"
        }`}
      >
        <span
          className={`absolute top-0.5 h-2.5 w-2.5 rounded-full bg-ink-950 transition-all ${
            checked ? "left-3.5" : "left-0.5"
          }`}
        />
      </span>
    </button>
  );
}
