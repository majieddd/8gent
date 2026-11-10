// The app's one way to the service: its control API under /8gent/v1. Inside the Tauri window calls go
// through the app's `api` command; in a plain browser (npm run dev) through Vite's proxy.
import { invoke } from "@tauri-apps/api/core";

export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export class ApiError extends Error {
  constructor(
    message: string,
    public status: number,
    public body: any,
  ) {
    super(message);
  }
}

/** The service is not answering at all (as opposed to answering with an error). */
export class Unreachable extends Error {}

async function call<T>(method: "GET" | "POST", path: string, body?: unknown): Promise<T> {
  let status: number;
  let text: string;
  try {
    if (inTauri) {
      const r = await invoke<{ status: number; body: string }>("api", { method, path, body: body === undefined ? null : JSON.stringify(body) });
      [status, text] = [r.status, r.body];
    } else {
      const r = await fetch(path, { method, headers: body === undefined ? undefined : { "Content-Type": "application/json" }, body: body === undefined ? undefined : JSON.stringify(body) });
      [status, text] = [r.status, await r.text()];
    }
  } catch (e) {
    throw new Unreachable(String(e));
  }
  let json: any = null;
  try {
    json = text ? JSON.parse(text) : null;
  } catch {
    // Vite's proxy answers a plain-text 500 when the service is down
    throw new Unreachable(text.slice(0, 200));
  }
  if (status >= 400) throw new ApiError(json?.error?.message ?? `The service answered ${status}.`, status, json);
  return json as T;
}

export interface Selection { loader: string; model: string; context: number; thinking: boolean }
export interface Running {
  loader: string; model: string; context: number; thinking: boolean; sessions: number; managed: boolean;
  base_url: string; engine_model: string; pid: number | null; loader_version: string | null; started_at: number; load_seconds: number;
}
export interface Speed { loader: string; model: string; requests: number; measured: number; mean_tokens_per_second: number | null; mean_first_token_ms: number | null }
export interface RequestRecord {
  at: number; loader: string; loader_version: string | null; model: string; path: string; stream: boolean; finish: string; status: number;
  prompt_tokens: number | null; completion_tokens: number | null; first_token_ms: number | null; duration_ms: number; tokens_per_second: number | null;
}
export interface Status {
  version: string; platform: string; os_version: string; port: number; share: boolean; key_set: boolean; served_name: string; home: string;
  memory_mode: "always" | "idle" | "on_demand"; idle_minutes: number; auto_switch: boolean; uptime_s: number; idle_s: number;
  selection: Selection;
  engine: { state: "stopped" | "loading" | "ready" | "error"; running: Running | null; memory_bytes: number | null; loading: { loader: string; model: string; seconds: number } | null; error: string | null };
  memory: { total_bytes: number | null; gpu_budget_bytes: number | null };
  also_loaded: { app: string; detail: string }[];
  /** Places models are read from that have not answered yet. */
  scanning: string[];
  stats: {
    in_flight: number; totals: { requests: number; errors: number; prompt_tokens: number; completion_tokens: number };
    overflows: { count: number; last_at: number | null; last_tokens: number | null; largest_tokens: number | null; context: number | null };
    speed: Speed[]; recent: RequestRecord[];
  };
}
export interface Capabilities { tools: boolean; json_schema: boolean; vision: boolean; anthropic_api: boolean; sessions: number; reserves_context: boolean }
export interface ModelLoader {
  id: string; name: string; kind: "managed" | "attach"; version: string | null; runnable: boolean; why_not: string | null; serving: boolean; selected: boolean;
  format: string; quant: string | null; variant: string | null; size_bytes: number | null; context: number | null; max_context: number | null;
  base_bytes: number | null; kv_bytes_per_token: number | null; sessions: number; estimated_memory_bytes: number | null;
  capabilities: Capabilities; speed: Speed | null;
}
export interface Model { id: string; name: string; selected: boolean; loaders: ModelLoader[] }
export interface Requirement { os: string; min: string; since: string | null; note: string; applies_to_installed: boolean; met: boolean }
export interface Loader {
  id: string; name: string; description: string; homepage: string; kind: "managed" | "attach"; formats: string[]; supported_here: boolean; installed: boolean;
  program: string | null; version: string | null; reachable: boolean | null; capabilities: Capabilities; requirements: Requirement[];
  variants: string[]; update: { method: string; source: string }; models: string[]; selected: boolean;
}

export const api = {
  status: () => call<Status>("GET", "/8gent/v1/status"),
  models: () => call<{ models: Model[] }>("GET", "/8gent/v1/models"),
  loaders: () => call<{ loaders: Loader[] }>("GET", "/8gent/v1/loaders"),
  requests: (limit = 200) => call<{ requests: RequestRecord[] }>("GET", `/8gent/v1/requests?limit=${limit}`),
  logs: (loader: string, lines = 400) => call<{ loader: string; lines: string[] }>("GET", `/8gent/v1/logs?loader=${encodeURIComponent(loader)}&lines=${lines}`),
  select: (body: { loader?: string; model?: string; context?: number; thinking?: boolean; load?: boolean }) => call<{ ok: boolean; loaded: boolean }>("POST", "/8gent/v1/select", body),
  start: () => call<{ ok: boolean }>("POST", "/8gent/v1/engine/start", {}),
  stop: () => call<{ ok: boolean }>("POST", "/8gent/v1/engine/stop", {}),
  rescan: () => call<{ ok: boolean; models: number }>("POST", "/8gent/v1/rescan", {}),
  settings: (body: Partial<Pick<Status, "memory_mode" | "idle_minutes" | "auto_switch">>) => call<{ ok: boolean }>("POST", "/8gent/v1/settings", body),
  shutdown: () => call<{ ok: boolean }>("POST", "/8gent/v1/shutdown", {}),
  /** Start the service if it is not running (only the app can; a browser tab cannot start programs). */
  startService: () => (inTauri ? invoke<string>("start_service") : Promise.reject(new Error("Run `8gent serve` in a terminal."))),
};
