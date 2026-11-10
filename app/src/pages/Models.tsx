import { Search } from "lucide-react";
import { useMemo, useState } from "react";
import { Button, Card, Chip, cx, Label, Meter, Notice, PageTitle, Segmented, Stat, State } from "../components/ui";
import { api, ApiError, Capabilities, Loader, Model, ModelLoader, Status } from "../lib/api";
import { gb, memoryAt, seconds, windowLabel, windowsFor } from "../lib/format";
import { Service } from "../lib/useService";

interface Props {
  svc: Service;
  status: Status;
  picked: string | null;
  setPicked: (id: string) => void;
}

const OS_NAMES: Record<string, string> = { macos: "macOS", windows: "Windows", linux: "Linux" };
const SOURCE_NAMES: Record<string, string> = { manual: "your own list", tensorfoldhome: "TensorFold Home's models", uzuhome: "uzu Home's models", lmstudio: "LM Studio's models" };

function formatLabel(l: ModelLoader): string {
  const quant = l.quant ? `, ${l.quant}` : "";
  switch (l.format) {
    case "gguf": return `GGUF${quant}`;
    case "mlx": return `MLX${quant}${l.variant === "tf06" ? " · runs on TensorFold 0.6.6" : ""}`;
    case "uzu": return "Folder converted for uzu";
    case "mirai": return `Mirai's build${quant}`;
    default: return `Served by ${l.name}`;
  }
}

function capabilityChips(c: Capabilities, sessions: number): string[] {
  const chips = [];
  if (c.tools) chips.push("Tools");
  if (c.json_schema) chips.push("JSON schema");
  if (c.vision) chips.push("Pictures");
  if (c.anthropic_api) chips.push("Anthropic API");
  if (c.sessions > 1) chips.push(sessions > 1 ? `Keeps ${sessions} conversations` : `Can keep ${c.sessions} conversations`);
  if (c.reserves_context) chips.push("Sets the whole window aside at load");
  return chips;
}

/** Notes a loader carries wherever it appears: an OS it needs that this computer does not have. */
export function requirementNotes(loader: Loader | undefined): string[] {
  if (!loader) return [];
  return loader.requirements.filter((r) => !r.met).map((r) => `${loader.name}${r.since ? ` from ${r.since}` : ""} needs ${OS_NAMES[r.os] ?? r.os} ${r.min}. ${r.note}`);
}

export function Models({ svc, status, picked, setPicked }: Props) {
  const [query, setQuery] = useState("");
  const [windows, setWindows] = useState<Record<string, number>>({});
  const [pending, setPending] = useState("");
  const [errors, setErrors] = useState<Record<string, { message: string; detail: string; canForce: boolean } | null>>({});

  // models 8gent can load itself come first; ones that only another app's server has follow
  const [own, served] = useMemo(() => {
    const q = query.trim().toLowerCase();
    const match = q ? svc.models.filter((m) => m.name.toLowerCase().includes(q) || m.id.includes(q)) : svc.models;
    const mine = (m: Model) => m.loaders.some((l) => l.kind === "managed");
    return [match.filter(mine), match.filter((m) => !mine(m))];
  }, [svc.models, query]);
  const model: Model | undefined = svc.models.find((m) => m.id === picked) ?? svc.models.find((m) => m.id === status.selection.model) ?? svc.models[0];
  const budget = status.memory.gpu_budget_bytes;
  const servingId = status.engine.running?.model ?? (status.engine.state === "loading" ? status.engine.loading?.model : undefined);

  const reading = status.scanning.map((s) => SOURCE_NAMES[s] ?? s);
  const stillReading = reading.length > 0 && (
    <Notice>Still reading {reading.join(" and ")}. If macOS is asking whether 8gent may open a folder, it is waiting for your answer; the models from there appear once you have given it.</Notice>
  );

  if (!model) {
    return (
      <>
        <PageTitle>Models</PageTitle>
        {stillReading || <p className="m-0 max-w-xl text-muted">No models found yet. 8gent looks in LM Studio's model folder, in what Ollama has pulled, and in a list you can write yourself (models.toml in its data folder).</p>}
      </>
    );
  }

  const managed = model.loaders.filter((l) => l.kind === "managed");
  const inUse = model.loaders.find((l) => l.serving) ?? model.loaders.find((l) => l.selected);
  const current = inUse?.context ?? managed[0]?.context ?? null;
  const maxWindow = managed.reduce<number | null>((max, l) => (l.max_context == null ? max : Math.max(max ?? 0, l.max_context)), null);
  const chosen = windows[model.id] ?? current ?? 65536;
  const running = status.engine.running;

  const serve = async (l: ModelLoader, force = false) => {
    const key = `${model.id}/${l.id}`;
    setPending(key);
    setErrors((e) => ({ ...e, [key]: null }));
    try {
      await api.select({ loader: l.id, model: model.id, context: l.kind === "managed" ? chosen : undefined, load: true, force });
    } catch (e) {
      setErrors((x) => ({ ...x, [key]: e instanceof ApiError ? { message: e.message, detail: e.body?.error?.detail ?? "", canForce: e.body?.can_force === true } : { message: "The service did not answer.", detail: "", canForce: false } }));
    }
    await svc.refresh();
    setPending("");
  };

  const row = (m: Model) => (
    <button
      key={m.id}
      type="button"
      aria-pressed={m.id === model.id}
      onClick={() => setPicked(m.id)}
      className={cx("flex min-h-[52px] items-center justify-between gap-2.5 rounded-lg border px-3 py-2 text-left transition-colors duration-fast ease-kr8", m.id === model.id ? "border-line bg-soft" : "border-transparent hover:bg-soft")}
    >
      <span className="flex min-w-0 flex-col">
        <span className="truncate font-medium">{m.name}</span>
        <span className="text-xs leading-4 text-muted">{m.loaders.length === 1 ? `Runs on ${m.loaders[0].name}` : `Runs on ${m.loaders.length} loaders`}</span>
      </span>
      {m.id === servingId && <span className="shrink-0 rounded-full bg-accent px-2 text-[11px] font-semibold leading-5 text-bg">In use</span>}
    </button>
  );

  return (
    <>
      <PageTitle
        actions={
          <label className="flex h-9 items-center gap-2 rounded-lg border border-line px-3 text-muted focus-within:border-accent">
            <Search aria-hidden className="h-[15px] w-[15px]" strokeWidth={1.75} />
            <span className="sr-only">Find a model</span>
            <input type="search" value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Find a model" className="w-48 border-0 bg-transparent text-fg outline-none placeholder:text-muted" />
          </label>
        }
      >
        Models
      </PageTitle>

      {stillReading}

      <div className="flex flex-wrap items-start gap-6">
        <div className="flex min-w-0 flex-[1_1_260px] flex-col gap-1">
          {own.map(row)}
          {served.length > 0 && <Label className="px-3 pb-1 pt-4">Served by other apps</Label>}
          {served.map(row)}
          {own.length + served.length === 0 && <p className="m-0 px-3 py-2 text-muted">No model matches "{query}".</p>}
        </div>

        <section aria-label="The model you picked" className="flex min-w-0 flex-[2.6_1_520px] flex-col gap-[18px]">
          <div className="flex flex-wrap items-end justify-between gap-3.5">
            <div className="flex min-w-0 flex-col gap-0.5">
              <h2 className="m-0 truncate text-xl font-semibold leading-[26px] tracking-[-0.01em]">{model.name}</h2>
              <span className="font-mono text-xs text-muted">{model.id}</span>
            </div>
            {managed.length > 0 && (
              <div className="flex flex-col gap-1.5">
                <span className="text-xs text-muted">Context window</span>
                <Segmented label="Context window" value={chosen} onChange={(w) => setWindows((x) => ({ ...x, [model.id]: w }))} options={windowsFor(maxWindow, current).map((w) => ({ value: w, label: windowLabel(w) }))} />
              </div>
            )}
          </div>

          <Label>Loaders that can run it</Label>

          {model.loaders.map((l) => {
            const key = `${model.id}/${l.id}`;
            const loader = svc.loaders.find((x) => x.id === l.id);
            const mem = memoryAt(l.base_bytes, l.kv_bytes_per_token, l.sessions, chosen, budget);
            const servingHere = l.serving && running != null;
            const sameWindow = !servingHere || !running.managed || running.context === chosen;
            const isPending = pending === key || (status.engine.state === "loading" && status.engine.loading?.model === model.id && status.engine.loading?.loader === l.id);
            const notes = requirementNotes(loader);
            const tps = l.speed?.mean_tokens_per_second;
            const overMax = l.max_context != null && chosen > l.max_context;
            return (
              <Card key={l.id} tone={servingHere ? "accent" : undefined} className="flex flex-col gap-4 px-5 py-[18px]">
                <div className="flex flex-wrap items-center justify-between gap-3">
                  <div className="flex flex-wrap items-baseline gap-2.5">
                    <span className="text-base font-semibold leading-[22px]">{l.name}</span>
                    {l.version && <span className="font-mono text-xs text-muted">{l.version}</span>}
                  </div>
                  {isPending ? (
                    <span className="flex h-9 items-center text-muted">Loading. If it fails, 8gent goes back.</span>
                  ) : servingHere && sameWindow ? (
                    <State tone="ok" className="h-9">Serving now</State>
                  ) : !l.runnable ? null : (
                    <Button variant="primary" onClick={() => serve(l)} disabled={pending !== "" || mem.fit === "too-big" || overMax}>
                      {servingHere ? `Reload at ${windowLabel(chosen)}` : l.selected ? "Load now" : l.format === "mirai" && !l.size_bytes ? `Download and switch to ${l.name}` : `Switch to ${l.name}`}
                    </Button>
                  )}
                </div>

                {!l.runnable && l.why_not && <p className="m-0 text-muted">{l.why_not}</p>}

                <div className="grid gap-5 [grid-template-columns:repeat(auto-fit,minmax(min(170px,100%),1fr))]">
                  <Stat
                    label="Speed on this computer"
                    value={tps ? `${tps.toFixed(1)} tokens/s` : "Not measured yet"}
                    note={tps ? `Over ${l.speed?.measured} ${l.speed?.measured === 1 ? "answer" : "answers"}${l.speed?.mean_first_token_ms ? ` · first token after ${seconds(l.speed.mean_first_token_ms)}` : ""}` : "Shown after a few longer answers"}
                  />
                  {l.kind === "managed" ? (
                    <Stat
                      label={`Memory at ${windowLabel(chosen)}`}
                      value={mem.bytes ? `About ${gb(mem.bytes)}` : "Not estimated yet"}
                      note={
                        overMax ? <span className="text-warn">This build allows at most {windowLabel(l.max_context!)}</span>
                        : mem.fit === "too-big" ? <span className="text-warn">Too big: this computer can hold {gb(budget)}</span>
                        : mem.fit === "tight" ? `Tight: this computer can hold ${gb(budget)}`
                        : mem.fit === "fits" ? `Fits: this computer can hold ${gb(budget)}`
                        : mem.bytes ? undefined : "Shown after its first load"
                      }
                      measured={l.measured ? `Measured: ${gb(l.measured.bytes)} at ${windowLabel(l.measured.context)}${l.measured.sessions > 1 ? ` with ${l.measured.sessions} conversations` : ""}` : undefined}
                    >
                      {mem.bytes && budget ? <div className="my-0.5"><Meter share={mem.share} tone={mem.fit === "too-big" ? "warn" : "accent"} /></div> : null}
                    </Stat>
                  ) : (
                    <Stat label="Memory and window" value={`Set by ${l.name}`} note="8gent cannot change them yet" />
                  )}
                  <Stat label="On disk" value={l.size_bytes ? gb(l.size_bytes) : l.kind === "attach" ? `In ${l.name}` : "Not downloaded"} note={formatLabel(l)} />
                </div>

                <div className="flex flex-wrap items-center gap-2">
                  {capabilityChips(l.capabilities, l.sessions).map((c) => <Chip key={c}>{c}</Chip>)}
                </div>

                {notes.map((n) => <Notice key={n}>{n}</Notice>)}
                {errors[key] && (
                  <Notice tone="bad" detail={errors[key]!.detail} action={errors[key]!.canForce ? <Button size="sm" onClick={() => serve(l, true)} disabled={pending !== ""}>Load anyway</Button> : undefined}>
                    {errors[key]!.message}
                  </Notice>
                )}
              </Card>
            );
          })}
        </section>
      </div>
    </>
  );
}
