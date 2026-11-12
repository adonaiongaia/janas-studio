import type { ButtonHTMLAttributes, ReactNode } from "react";
import { Loader2 } from "lucide-react";

import { cn } from "../../lib/cn";

type Variant = "primary" | "secondary" | "ghost" | "danger";
type Size = "sm" | "md" | "icon";

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: Variant;
  size?: Size;
  busy?: boolean;
  icon?: ReactNode;
}

const VARIANTS: Record<Variant, string> = {
  primary:
    "bg-acid-400 text-ink-950 hover:bg-acid-300 active:bg-acid-500 shadow-glow",
  secondary:
    "border border-white/10 bg-white/5 text-slate-200 hover:border-white/20 hover:bg-white/10",
  ghost: "text-slate-400 hover:bg-white/5 hover:text-slate-200",
  danger:
    "border border-rose-400/25 bg-rose-500/10 text-rose-300 hover:bg-rose-500/20",
};

const SIZES: Record<Size, string> = {
  sm: "h-8 gap-1.5 rounded-lg px-3 text-xs",
  md: "h-10 gap-2 rounded-xl px-4 text-[13px]",
  icon: "h-8 w-8 justify-center rounded-lg",
};

export function Button({
  variant = "secondary",
  size = "md",
  busy = false,
  icon,
  children,
  className,
  disabled,
  type = "button",
  ...props
}: ButtonProps) {
  return (
    <button
      type={type}
      disabled={disabled || busy}
      className={cn(
        "inline-flex select-none items-center font-medium transition focus:outline-none focus-visible:ring-2 focus-visible:ring-acid-400/40 disabled:cursor-not-allowed disabled:opacity-50",
        VARIANTS[variant],
        SIZES[size],
        className,
      )}
      {...props}
    >
      {busy ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : icon}
      {children}
    </button>
  );
}
