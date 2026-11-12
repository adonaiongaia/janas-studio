interface SliderProps {
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  onChange: (value: number) => void;
  formatValue?: (value: number) => string;
  leftHint?: string;
  rightHint?: string;
  description?: string;
  disabled?: boolean;
}

export function Slider({
  label,
  value,
  min,
  max,
  step,
  onChange,
  formatValue = (current) => current.toFixed(2),
  leftHint,
  rightHint,
  description,
  disabled = false,
}: SliderProps) {
  return (
    <div className="space-y-2.5">
      <div className="flex items-center justify-between">
        <span className="text-xs font-medium text-slate-300">{label}</span>
        <span className="rounded-md border border-white/10 bg-white/5 px-2 py-0.5 font-mono text-[11px] text-acid-200">
          {formatValue(value)}
        </span>
      </div>
      <input
        type="range"
        className="range-acid"
        min={min}
        max={max}
        step={step}
        value={value}
        disabled={disabled}
        onChange={(event) => onChange(Number(event.target.value))}
        aria-label={label}
      />
      <div className="flex items-center justify-between text-[10px] text-slate-600">
        <span>{leftHint}</span>
        <span>{rightHint}</span>
      </div>
      {description ? (
        <p className="text-[11px] leading-5 text-slate-500">{description}</p>
      ) : null}
    </div>
  );
}
