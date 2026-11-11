import { Activity, Bot, Boxes, Check, Copy, Cpu, Home as HomeIcon, List, MessageSquare, ScrollText, SlidersHorizontal } from "lucide-react";
import { useEffect, useState } from "react";
import { Button, cx, Mark8, State } from "./components/ui";
import { api, inTauri, Status } from "./lib/api";
import { Service, useService } from "./lib/useService";
import { Chat } from "./pages/Chat";
import { Home } from "./pages/Home";
import { Harnesses } from "./pages/Harnesses";
import { Loaders } from "./pages/Loaders";
import { Logs } from "./pages/Logs";
import { Models } from "./pages/Models";
import { Monitor } from "./pages/Monitor";
import { Requests } from "./pages/Requests";
import { Settings } from "./pages/Settings";

export type Tab = "home" | "chat" | "models" | "loaders" | "harnesses" | "monitor" | "requests" | "logs" | "settings";
export type Theme = "system" | "light" | "dark";

const TABS: { id: Tab; label: string; icon: typeof HomeIcon }[] = [
  { id: "home", label: "Home", icon: HomeIcon },
  { id: "chat", label: "Chat", icon: MessageSquare },
  { id: "models", label: "Models", icon: Boxes },
  { id: "loaders", label: "Loaders", icon: Cpu },
  { id: "harnesses", label: "Harnesses", icon: Bot },
  { id: "monitor", label: "Monitor", icon: Activity },
  { id: "requests", label: "Requests", icon: List },
  { id: "logs", label: "Logs", icon: ScrollText },
  { id: "settings", label: "Settings", icon: SlidersHorizontal },
];

function useTheme(): [Theme, (t: Theme) => void] {
  const [theme, setTheme] = useState<Theme>(() => {
    try {
      return (localStorage.getItem("theme") as Theme) || "system";
    } catch {
      return "system";
    }
  });
  useEffect(() => {
    const system = window.matchMedia("(prefers-color-scheme: dark)");
    const apply = () => document.documentElement.classList.toggle("dark", theme === "dark" || (theme === "system" && system.matches));
    apply();
    system.addEventListener("change", apply);
    try {
      localStorage.setItem("theme", theme);
    } catch {
      // the choice then lasts for this window only
    }
    return () => system.removeEventListener("change", apply);
  }, [theme]);
  return [theme, setTheme];
}

/** What the service is doing, in a dot and a few words. */
export function engineState(status: Status, modelName: (id: string) => string, loaderName: (id: string) => string): { tone: "ok" | "warn" | "bad" | "off"; word: string; detail: string } {
  const e = status.engine;
  if (e.state === "ready" && e.running) return { tone: "ok", word: "Ready", detail: `${modelName(e.running.model)} on ${loaderName(e.running.loader)}` };
  if (e.state === "loading" && e.loading) return { tone: "warn", word: "Loading", detail: `${modelName(e.loading.model)} on ${loaderName(e.loading.loader)}` };
  if (e.state === "error") return { tone: "bad", word: "Problem", detail: "See Home for the reason" };
  if (status.selection.model) return { tone: "off", word: "Not loaded", detail: `${modelName(status.selection.model)} on ${loaderName(status.selection.loader)}` };
  return { tone: "off", word: "Nothing selected", detail: "Choose a model" };
}

export function CopyButton({ text, label, className }: { text: string; label: string; className?: string }) {
  const [done, setDone] = useState(false);
  return (
    <button
      type="button"
      aria-label={label}
      onClick={() => navigator.clipboard.writeText(text).then(() => { setDone(true); setTimeout(() => setDone(false), 1500); }, () => {})}
      className={cx("inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-sm text-current opacity-70 transition-opacity duration-fast hover:opacity-100", className)}
    >
      {done ? <Check aria-hidden className="h-[15px] w-[15px]" strokeWidth={1.75} /> : <Copy aria-hidden className="h-[15px] w-[15px]" strokeWidth={1.75} />}
    </button>
  );
}

function ServiceDown({ onStarted }: { onStarted: () => void }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const start = async () => {
    setBusy(true);
    setError("");
    try {
      await api.startService();
      onStarted();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
    setBusy(false);
  };
  return (
    <div className="flex h-full flex-col items-center justify-center gap-4 px-8 text-center">
      <div className="flex items-center gap-1">
        <Mark8 className="h-9 w-[21px]" />
        <span className="text-[30px] font-semibold tracking-[-0.02em]">gent</span>
      </div>
      <p className="m-0 max-w-md text-muted">The 8gent service is not running. It is the part that keeps the endpoint up and loads your models; this window only shows it.</p>
      {inTauri ? (
        <Button variant="primary" onClick={start} disabled={busy}>{busy ? "Starting" : "Start the service"}</Button>
      ) : (
        <code className="rounded-md bg-soft px-3 py-2 font-mono text-[13px]">8gent serve</code>
      )}
      {error && <p className="m-0 max-w-md text-bad">{error}</p>}
    </div>
  );
}

export function App() {
  const svc: Service = useService();
  const [tab, setTab] = useState<Tab>("home");
  const [picked, setPicked] = useState<string | null>(null);
  const [theme, setTheme] = useTheme();
  const { status } = svc;
  const modelName = (id: string) => svc.models.find((m) => m.id === id)?.name ?? id;
  const endpoint = `http://127.0.0.1:${status?.port ?? 8436}/v1`;
  const loaderName = (id: string) => svc.loaders.find((l) => l.id === id)?.name ?? id;
  const state = status ? engineState(status, modelName, loaderName) : null;

  const openModels = (model?: string) => {
    if (model) setPicked(model);
    setTab("models");
  };

  return (
    <div className="flex h-full min-h-0 text-sm leading-5">
      <nav aria-label="Sections" className="chrome flex w-[232px] shrink-0 flex-col gap-5 px-3 pb-4 pt-[52px]" data-tauri-drag-region>
        <div className="flex items-center gap-[3px] px-2" data-tauri-drag-region>
          <Mark8 className="h-[26px] w-[15px]" />
          <span className="text-[21px] font-semibold leading-[26px] tracking-[-0.02em]">gent</span>
        </div>
        <div className="flex flex-col gap-0.5">
          {TABS.map(({ id, label, icon: Icon }) => (
            <button
              key={id}
              type="button"
              aria-current={tab === id ? "page" : undefined}
              onClick={() => setTab(id)}
              className={cx("flex h-[34px] items-center gap-2.5 rounded-md px-2.5 text-left font-medium transition-colors duration-fast ease-kr8", tab === id ? "bg-chrome-sel text-chrome-fg" : "text-chrome-muted hover:bg-chrome-sel")}
            >
              <Icon aria-hidden className="h-4 w-4" strokeWidth={1.75} />
              <span>{label}</span>
            </button>
          ))}
        </div>
        <div className="mt-auto flex flex-col gap-2 rounded-lg bg-chrome-sel px-2.5 py-3">
          {state ? (
            <>
              <State tone={state.tone}>{state.word}</State>
              <div className="truncate text-[13px] text-chrome-muted" title={state.detail}>{state.detail}</div>
            </>
          ) : (
            <State tone="off">{svc.down ? "Service not running" : "Connecting"}</State>
          )}
          <div className="flex items-center justify-between gap-1">
            <span className="truncate font-mono text-xs" title={endpoint}>{endpoint.replace("http://", "")}</span>
            <CopyButton text={endpoint} label="Copy the endpoint address" />
          </div>
        </div>
      </nav>

      <main className="min-w-0 flex-1 overflow-y-auto">
        {svc.down || !status ? (
          svc.down ? <ServiceDown onStarted={svc.refresh} /> : null
        ) : tab === "chat" ? (
          <div className="mx-auto flex h-full max-w-[1180px] flex-col px-8 pb-6 pt-7">
            <Chat svc={svc} status={status} modelName={modelName} loaderName={loaderName} openModels={() => openModels()} />
          </div>
        ) : (
          <div className="mx-auto flex max-w-[1180px] flex-col gap-6 px-8 pb-10 pt-7">
            {tab === "home" && <Home svc={svc} status={status} modelName={modelName} loaderName={loaderName} openModels={openModels} openRequests={() => setTab("requests")} />}
            {tab === "models" && <Models svc={svc} status={status} picked={picked} setPicked={setPicked} />}
            {tab === "loaders" && <Loaders svc={svc} status={status} />}
            {tab === "harnesses" && <Harnesses />}
            {tab === "monitor" && <Monitor status={status} modelName={modelName} loaderName={loaderName} />}
            {tab === "requests" && <Requests modelName={modelName} loaderName={loaderName} />}
            {tab === "logs" && <Logs svc={svc} status={status} />}
            {tab === "settings" && <Settings svc={svc} status={status} theme={theme} setTheme={setTheme} endpoint={endpoint} />}
          </div>
        )}
      </main>
    </div>
  );
}
