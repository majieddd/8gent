// The app's one way to the service: its control API under /8gent/v1. Inside the Tauri window calls go
// through the app's `api` command; in a plain browser (npm run dev) through Vite's proxy.
import { Channel, invoke } from "@tauri-apps/api/core";

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

async function call<T>(method: "GET" | "POST" | "PUT" | "DELETE", path: string, body?: unknown): Promise<T> {
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
  version: string; platform: string; os_version: string; port: number; also_listening: number[]; login_item: boolean; share: boolean; key_set: boolean; served_name: string; home: string;
  memory_mode: "always" | "idle" | "on_demand"; idle_minutes: number; auto_switch: boolean; auto_update: boolean; uptime_s: number; idle_s: number;
  selection: Selection;
  engine: { state: "stopped" | "loading" | "ready" | "error"; running: Running | null; memory_bytes: number | null; loading: { loader: string; model: string; seconds: number } | null; error: string | null };
  memory: { total_bytes: number | null; gpu_budget_bytes: number | null; available_bytes: number | null; holders: { name: string; detail: string; bytes: number | null }[] };
  also_loaded: { app: string; detail: string }[];
  /** The small model kept loaded next to the selected one, answering under `name`. */
  small: { selection: Selection; name: string; state: "stopped" | "loading" | "ready" | "error"; running: Running | null; memory_bytes: number | null; error: string | null };
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
  /** The most memory this pair was seen to hold, and at which window. */
  measured: { context: number; sessions: number; bytes: number } | null;
}
export interface Model { id: string; name: string; selected: boolean; loaders: ModelLoader[] }
export interface Requirement { os: string; min: string; since: string | null; note: string; applies_to_installed: boolean; met: boolean }
export interface Loader {
  id: string; name: string; description: string; homepage: string; kind: "managed" | "attach"; formats: string[]; supported_here: boolean; installed: boolean;
  program: string | null; version: string | null; reachable: boolean | null; capabilities: Capabilities; requirements: Requirement[];
  variants: string[]; update: { method: string; source: string }; models: string[]; selected: boolean;
}

export interface Sample { at: number; tokens_per_second: number; in_flight: number; memory_bytes: number | null; state: Status["engine"]["state"] }
export interface ChatMessage {
  role: "user" | "assistant";
  content: string;
  /** The model's reasoning, shown folded away above its answer. */
  thinking?: string;
  thinking_ms?: number;
  tokens?: number;
  tokens_per_second?: number;
  first_token_ms?: number;
  /** What answered: "OxCoder-9B on uzu". */
  by?: string;
  error?: string;
  stopped?: boolean;
}
export interface ChatDoc { id: string; title: string; updated_at?: number; messages: ChatMessage[] }
export interface ChatRow { id: string; title: string; updated_at: number; messages: number }

/**
 * One streamed chat request to the endpoint. `onEvent` gets each event of the stream as it arrives.
 * Aborting `signal` stops the answer. A refusal (a prompt that does not fit, no model selected)
 * rejects with the service's own message.
 */
export async function streamChat(body: object, onEvent: (event: any) => void, signal: AbortSignal): Promise<void> {
  let pending = "";
  let raw = "";
  const push = (text: string) => {
    raw = raw.length < 20000 ? raw + text : raw;
    pending += text;
    let nl: number;
    while ((nl = pending.indexOf("\n")) >= 0) {
      const line = pending.slice(0, nl).replace(/\r$/, "");
      pending = pending.slice(nl + 1);
      if (!line.startsWith("data:")) continue;
      const payload = line.slice(5).trim();
      if (payload && payload !== "[DONE]") {
        try {
          onEvent(JSON.parse(payload));
        } catch {
          // a line that is not JSON carries nothing for the chat
        }
      }
    }
  };
  let status: number;
  try {
    if (inTauri) {
      const id = crypto.randomUUID();
      const channel = new Channel<string>();
      channel.onmessage = push;
      const stop = () => void invoke("chat_cancel", { id });
      signal.addEventListener("abort", stop);
      try {
        status = await invoke<number>("chat_stream", { id, body: JSON.stringify(body), onChunk: channel });
      } finally {
        signal.removeEventListener("abort", stop);
      }
    } else {
      const r = await fetch("/v1/chat/completions", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body), signal });
      status = r.status;
      const reader = r.body!.getReader();
      const decoder = new TextDecoder();
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        push(decoder.decode(value, { stream: true }));
      }
    }
  } catch (e) {
    if (signal.aborted) return;
    throw new Unreachable(String(e));
  }
  if (status >= 400) {
    let json: any = null;
    try {
      json = JSON.parse(raw);
    } catch {
      // the message below stands in
    }
    throw new ApiError(json?.error?.message ?? `The service answered ${status}.`, status, json);
  }
}

export interface UpdateState {
  loader: string; current: string | null; latest: string | null;
  status: "current" | "available" | "held" | "not_installed" | "elsewhere" | "unknown";
  note: string; checked_at: number | null; busy: boolean; last_result: string | null;
}

export interface AppUpdate { current: string; latest: string; available: boolean; can_install: boolean; page: string }

/** The app's own updates: only the window can replace itself, so these exist inside it alone. */
export const appUpdate = {
  check: () => (inTauri ? invoke<AppUpdate>("app_update_check") : Promise.reject(new Error("Only the app can update itself."))),
  install: () => invoke<string>("app_update_install"),
};

export type HarnessLink =
  | { state: "connected"; entries: string[] }
  | { state: "not_connected" }
  | { state: "unreadable"; why: string }
  | { state: "not_by_eightgent"; why: string };
export interface Harness {
  id: string; name: string; summary: string; homepage: string;
  installed: boolean; program: string | null; version: string | null;
  link: HarnessLink;
  /** 8gent can write this harness's settings (otherwise `link.why` says how it is set). */
  can_connect: boolean;
  /** "settings": 8gent writes its settings; "own": it chooses its endpoint itself; "none": it cannot use this endpoint. */
  connect_kind: "settings" | "own" | "none";
  settings_file: string | null;
  /** A command that uses the harness with 8gent, to copy. */
  use: string;
  runs_as_agent: boolean;
}
export interface HarnessWritten { changed: boolean; file: string; backup: string | null }

export const api = {
  harnesses: () => call<{ endpoint: string; harnesses: Harness[] }>("GET", "/8gent/v1/harnesses"),
  connectHarness: (id: string, on: boolean) => call<{ ok: boolean; written: HarnessWritten; harness: Harness }>("POST", `/8gent/v1/harnesses/${id}/${on ? "connect" : "disconnect"}`, {}),
  updates: () => call<{ updates: UpdateState[]; auto_update: boolean }>("GET", "/8gent/v1/updates"),
  checkUpdates: () => call<{ updates: UpdateState[]; auto_update: boolean }>("POST", "/8gent/v1/updates/check", {}),
  updateLoader: (id: string) => call<{ ok: boolean }>("POST", `/8gent/v1/loaders/${id}/update`, {}),
  monitor: () => call<{ samples: Sample[]; gpu_budget_bytes: number | null; total_bytes: number | null }>("GET", "/8gent/v1/monitor"),
  chats: () => call<{ chats: ChatRow[] }>("GET", "/8gent/v1/chats"),
  chat: (id: string) => call<ChatDoc>("GET", `/8gent/v1/chats/${id}`),
  saveChat: (doc: ChatDoc) => call<{ ok: boolean }>("PUT", `/8gent/v1/chats/${doc.id}`, doc),
  deleteChat: (id: string) => call<{ ok: boolean }>("DELETE", `/8gent/v1/chats/${id}`),
  status: () => call<Status>("GET", "/8gent/v1/status"),
  models: () => call<{ models: Model[] }>("GET", "/8gent/v1/models"),
  loaders: () => call<{ loaders: Loader[] }>("GET", "/8gent/v1/loaders"),
  requests: (limit = 200) => call<{ requests: RequestRecord[] }>("GET", `/8gent/v1/requests?limit=${limit}`),
  logs: (loader: string, lines = 400) => call<{ loader: string; lines: string[] }>("GET", `/8gent/v1/logs?loader=${encodeURIComponent(loader)}&lines=${lines}`),
  select: (body: { loader?: string; model?: string; context?: number; thinking?: boolean; load?: boolean; force?: boolean }) => call<{ ok: boolean; loaded: boolean }>("POST", "/8gent/v1/select", body),
  start: (force = false) => call<{ ok: boolean }>("POST", "/8gent/v1/engine/start", { force }),
  stop: () => call<{ ok: boolean }>("POST", "/8gent/v1/engine/stop", {}),
  rescan: () => call<{ ok: boolean; models: number }>("POST", "/8gent/v1/rescan", {}),
  settings: (body: Partial<Pick<Status, "memory_mode" | "idle_minutes" | "auto_switch" | "auto_update">>) => call<{ ok: boolean }>("POST", "/8gent/v1/settings", body),
  shutdown: () => call<{ ok: boolean }>("POST", "/8gent/v1/shutdown", {}),
  small: (body: { model: string; loader?: string; context?: number } | { off: true }) => call<{ ok: boolean; loaded: boolean }>("POST", "/8gent/v1/small", body),
  loginItem: (enabled: boolean) => call<{ ok: boolean; restarting?: boolean }>("POST", "/8gent/v1/login-item", { enabled }),
  /** Start the service if it is not running (only the app can; a browser tab cannot start programs). */
  startService: () => (inTauri ? invoke<string>("start_service") : Promise.reject(new Error("Run `8gent serve` in a terminal."))),
};
