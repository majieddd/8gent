// The few building blocks every screen uses, in kr8's shapes: 10 px corners, hairline borders,
// near-black (or near-white) primary buttons, uppercase tracked section labels.
import { AlertTriangle } from "lucide-react";
import type { ButtonHTMLAttributes, ReactNode } from "react";

const cx = (...parts: (string | false | null | undefined)[]) => parts.filter(Boolean).join(" ");

export function Button({ variant = "outline", size = "md", className, ...rest }: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: "primary" | "outline" | "ghost"; size?: "md" | "sm" }) {
  return (
    <button
      type="button"
      {...rest}
      className={cx(
        "inline-flex shrink-0 items-center justify-center gap-1.5 rounded-lg font-medium transition-colors duration-fast ease-kr8 disabled:cursor-not-allowed disabled:opacity-50",
        size === "md" ? "h-9 px-4" : "h-8 px-3 text-[13px]",
        variant === "primary" && "bg-primary text-on-primary hover:opacity-90",
        variant === "outline" && "border border-line bg-transparent text-fg hover:bg-soft",
        variant === "ghost" && "bg-transparent text-muted hover:bg-soft hover:text-fg",
        className,
      )}
    />
  );
}

export function Card({ className, children, tone }: { className?: string; children: ReactNode; tone?: "accent" }) {
  return <div className={cx("rounded-xl border bg-card", tone === "accent" ? "border-accent" : "border-line", className)}>{children}</div>;
}

/** An uppercase, tracked label above a group. */
export function Label({ children, className }: { children: ReactNode; className?: string }) {
  return <div className={cx("text-xs font-semibold uppercase tracking-[0.06em] text-muted", className)}>{children}</div>;
}

export function Chip({ children }: { children: ReactNode }) {
  return <span className="inline-flex h-6 items-center rounded-full border border-line px-2.5 text-xs text-muted">{children}</span>;
}

type Tone = "ok" | "warn" | "bad" | "off" | "accent";
const toneText: Record<Tone, string> = { ok: "text-ok", warn: "text-warn", bad: "text-bad", off: "text-muted", accent: "text-accent" };
const toneFill: Record<Tone, string> = { ok: "bg-ok", warn: "bg-warn", bad: "bg-bad", off: "bg-muted", accent: "bg-accent" };

/** A coloured dot with words: the state of something. */
export function State({ tone, children, className }: { tone: Tone; children: ReactNode; className?: string }) {
  return (
    <span className={cx("inline-flex items-center gap-2 font-medium", toneText[tone], className)}>
      <span aria-hidden className={cx("h-2 w-2 shrink-0 rounded-full", toneFill[tone], tone === "warn" && "animate-pulse")} />
      <span>{children}</span>
    </span>
  );
}

export function Meter({ share, tone = "accent" }: { share: number; tone?: "accent" | "warn" }) {
  return (
    <div aria-hidden className="h-1.5 rounded-full bg-track">
      <div className={cx("h-1.5 rounded-full transition-[width] duration-slow ease-kr8", tone === "warn" ? "bg-warn" : "bg-accent")} style={{ width: `${Math.max(0, Math.min(100, share * 100))}%` }} />
    </div>
  );
}

/** A choice between a few values, kr8's segmented control. */
export function Segmented<T extends string | number>({ label, options, value, onChange }: { label: string; options: { value: T; label: string }[]; value: T; onChange: (v: T) => void }) {
  return (
    <div role="group" aria-label={label} className="inline-flex gap-0.5 rounded-md bg-soft p-[3px]">
      {options.map((o) => (
        <button
          key={String(o.value)}
          type="button"
          aria-pressed={o.value === value}
          onClick={() => onChange(o.value)}
          className={cx("h-7 rounded-sm px-3 text-[13px] font-medium transition-colors duration-fast ease-kr8", o.value === value ? "bg-card text-fg shadow-sm" : "text-muted hover:text-fg")}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

/** A note that needs attention, with an optional action on the right and, under it, the engine's
 *  own words when there are any. */
export function Notice({ children, action, tone = "warn", detail }: { children: ReactNode; action?: ReactNode; tone?: "warn" | "bad"; detail?: string }) {
  return (
    <div className="flex flex-col gap-2.5 rounded-lg bg-soft px-3.5 py-3">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className={cx("flex min-w-0 items-start gap-2.5", tone === "bad" ? "text-bad" : "text-warn")}>
          <AlertTriangle aria-hidden className="mt-0.5 h-4 w-4 shrink-0" strokeWidth={1.75} />
          <span className="min-w-0 whitespace-pre-line">{children}</span>
        </div>
        {action}
      </div>
      {detail && (
        <pre tabIndex={0} className="m-0 max-h-40 overflow-auto whitespace-pre-wrap break-words rounded-md border border-line bg-card px-3 py-2 font-mono text-xs leading-[18px] text-muted">{detail}</pre>
      )}
    </div>
  );
}

export function Stat({ label, value, note, measured, children }: { label: string; value: ReactNode; note?: ReactNode; measured?: string; children?: ReactNode }) {
  return (
    <div className="flex min-w-0 flex-col gap-1">
      <span className="text-xs text-muted">{label}</span>
      <span className="text-lg font-semibold leading-6">{value}</span>
      {children}
      {note && <span className="text-xs text-muted">{note}</span>}
      {measured && <span className="text-xs text-muted">{measured}</span>}
    </div>
  );
}

export function PageTitle({ children, actions }: { children: ReactNode; actions?: ReactNode }) {
  return (
    <div className="flex flex-wrap items-center justify-between gap-3">
      <h1 className="m-0 text-[22px] font-semibold leading-7 tracking-[-0.01em]">{children}</h1>
      {actions && <div className="flex flex-wrap items-center gap-2.5">{actions}</div>}
    </div>
  );
}

/** kr8's rainbow "8": two stroked circles under its ten-stop gradient. */
export function Mark8({ className }: { className?: string }) {
  const stops: [number, string][] = [[0, "#fb3e09"], [0.14, "#fb9401"], [0.28, "#f7c602"], [0.42, "#b6d001"], [0.54, "#57c730"], [0.66, "#1ab88b"], [0.76, "#10b2bc"], [0.86, "#0596f6"], [0.94, "#5041f7"], [1, "#911fe3"]];
  return (
    <svg viewBox="0 0 20 34" role="img" aria-label="8" className={className} fill="none">
      <defs>
        <linearGradient id="mark8" gradientUnits="userSpaceOnUse" x1="4" y1="0" x2="16" y2="34">
          {stops.map(([offset, color]) => (
            <stop key={offset} offset={offset} stopColor={color} />
          ))}
        </linearGradient>
      </defs>
      <circle cx="10" cy="9.4" r="6.9" stroke="url(#mark8)" strokeWidth="5" />
      <circle cx="10" cy="23.8" r="7.8" stroke="url(#mark8)" strokeWidth="4.4" />
    </svg>
  );
}

export { cx };
