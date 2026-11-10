export const gb = (bytes: number | null | undefined, digits = 1) => (bytes == null ? "" : `${(bytes / 1e9).toFixed(digits)} GB`);

/** 65536 -> "64K", 1048576 -> "1M": context sizes count in 1024s. */
export function windowLabel(tokens: number): string {
  if (tokens >= 1048576 && tokens % 1048576 === 0) return `${tokens / 1048576}M`;
  if (tokens % 1024 === 0) return `${tokens / 1024}K`;
  return tokens.toLocaleString("en-US");
}

export const count = (n: number | null | undefined) => (n == null ? "" : n.toLocaleString("en-US"));

export const WINDOWS = [16384, 32768, 65536, 131072, 262144, 524288, 1048576];

/** The windows to offer for a model: the usual sizes up to its maximum, always including the current one. */
export function windowsFor(max: number | null, current: number | null): number[] {
  const upTo = max ?? 262144;
  const list = WINDOWS.filter((w) => w <= upTo && (w <= 262144 || upTo > 262144));
  if (current && !list.includes(current)) list.push(current);
  return list.sort((a, b) => a - b);
}

/** A window with room for a prompt of `tokens`: the next usual size at least a quarter larger. */
export function windowFor(tokens: number, max: number | null): number | null {
  return WINDOWS.find((w) => w >= tokens * 1.25 && (max == null || w <= max)) ?? null;
}

export function clock(unixSeconds: number): string {
  return new Date(unixSeconds * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
}

export function seconds(ms: number | null | undefined): string {
  if (ms == null) return "";
  return ms < 1000 ? `${Math.round(ms)} ms` : `${(ms / 1000).toFixed(1)} s`;
}

export type Fit = "fits" | "tight" | "too-big" | "unknown";

/** Memory a loader needs for a window, and how that sits against what this computer's GPU can hold. */
export function memoryAt(base: number | null, kv: number | null, sessions: number, context: number, budget: number | null): { bytes: number | null; fit: Fit; share: number } {
  if (base == null || kv == null) return { bytes: null, fit: "unknown", share: 0 };
  const bytes = base + kv * context * Math.max(1, sessions);
  if (!budget) return { bytes, fit: "unknown", share: 0 };
  const share = bytes / budget;
  return { bytes, share, fit: share > 1 ? "too-big" : share > 0.85 ? "tight" : "fits" };
}
