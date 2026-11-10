import { useState } from "react";
import { Button, Card, Label, Meter, Notice, PageTitle, Stat, State } from "../components/ui";
import { api, ApiError, Status } from "../lib/api";
import { clock, count, gb, seconds, windowFor, windowLabel } from "../lib/format";
import { Service } from "../lib/useService";

interface Props {
  svc: Service;
  status: Status;
  modelName: (id: string) => string;
  loaderName: (id: string) => string;
  openModels: (model?: string) => void;
  openRequests: () => void;
}

export function Home({ svc, status, modelName, loaderName: nameOf, openModels, openRequests }: Props) {
  const [busy, setBusy] = useState("");
  const [error, setError] = useState("");
  const { engine, selection, stats, memory } = status;
  const run = engine.running;
  const target = run ?? (engine.loading ? { ...selection, ...engine.loading } : selection.model ? selection : null);
  const model = target ? svc.models.find((m) => m.id === target.model) : undefined;
  const pair = model?.loaders.find((l) => l.id === target?.loader);
  const loaderName = pair?.name ?? (target ? nameOf(target.loader) : "");

  const act = async (name: string, fn: () => Promise<unknown>) => {
    setBusy(name);
    setError("");
    try {
      await fn();
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "The service did not answer.");
    }
    await svc.refresh();
    setBusy("");
  };

  // a prompt that did not fit this window: offer the next size with room for it
  const tooLong = stats.overflows;
  const window_ = run?.context ?? pair?.context ?? null;
  const showTooLong = tooLong.count > 0 && window_ != null && tooLong.context === window_;
  const bigger = showTooLong && tooLong.largest_tokens ? windowFor(tooLong.largest_tokens, pair?.max_context ?? null) : null;

  const speed = pair?.speed?.mean_tokens_per_second;
  const budget = memory.gpu_budget_bytes;
  const held = engine.memory_bytes;

  return (
    <>
      <PageTitle>Home</PageTitle>

      {memory.holders.length > 0 && (
        <Notice>
          Also holding memory for models right now: {memory.holders.map((h) => `${h.name} (${h.detail}${h.bytes ? `, ${gb(h.bytes)}` : ""})`).join(" and ")}. Two loaded models may not fit in this computer's memory together.
        </Notice>
      )}

      <Card className="flex flex-col gap-6 p-7">
        <div className="flex flex-wrap items-start justify-between gap-5">
          <div className="flex min-w-0 flex-col gap-2">
            <Label>{run ? "Now serving" : engine.state === "loading" ? "Loading" : "Selected"}</Label>
            {target ? (
              <>
                <h2 className="m-0 truncate text-[34px] font-semibold leading-10 tracking-[-0.02em]">{modelName(target.model)}</h2>
                <span className="text-muted">
                  on {loaderName}
                  {run?.loader_version ? ` ${run.loader_version}` : pair?.version ? ` ${pair.version}` : ""}
                  {run?.managed ? ` · ${count(run.context)}-token window` : !run && pair?.context ? ` · ${count(pair.context)}-token window` : ""}
                  {run && run.sessions > 1 ? ` · ${run.sessions} conversations` : ""}
                </span>
              </>
            ) : (
              <h2 className="m-0 text-[28px] font-semibold leading-9 tracking-[-0.02em]">Nothing selected yet</h2>
            )}
            {engine.state === "ready" && run && <State tone="ok">Ready · loaded in {run.load_seconds} s</State>}
            {engine.state === "loading" && <State tone="warn">Loading, {engine.loading?.seconds ?? 0} s so far</State>}
            {engine.state === "stopped" && target && <State tone="off">Not loaded. The next request loads it.</State>}
            {engine.state === "error" && <State tone="bad">It did not load</State>}
          </div>
          <div className="flex flex-wrap gap-2.5">
            <Button variant="primary" onClick={() => openModels(target?.model)}>{target ? "Change model or loader" : "Choose a model"}</Button>
            {run && <Button onClick={() => act("stop", api.stop)} disabled={busy !== ""}>{busy === "stop" ? "Unloading" : "Unload"}</Button>}
            {!run && target && engine.state !== "loading" && <Button onClick={() => act("start", () => api.start(error.includes("load anyway")))} disabled={busy !== ""}>{busy === "start" ? "Loading" : error.includes("load anyway") ? "Load anyway" : "Load now"}</Button>}
          </div>
        </div>

        {engine.state === "error" && engine.error && <Notice tone="bad" detail={engine.error.split("\n\n")[1]}>{engine.error.split("\n\n")[0]}</Notice>}
        {error && <Notice tone="bad">{error}</Notice>}

        {target && (
          <div className="grid gap-6 [grid-template-columns:repeat(auto-fit,minmax(min(220px,100%),1fr))]">
            <Stat
              label="Speed on this computer"
              value={speed ? `${speed.toFixed(1)} tokens/s` : "Not measured yet"}
              note={speed ? `Measured over ${pair?.speed?.measured} ${pair?.speed?.measured === 1 ? "answer" : "answers"}` : "Shown after a few longer answers"}
            />
            <Stat
              label="Memory"
              value={held ? gb(held) : pair?.estimated_memory_bytes ? `About ${gb(pair.estimated_memory_bytes)}` : run && !run.managed ? `Held by ${loaderName}` : "Not measured yet"}
              note={held && budget ? `of the ${gb(budget)} this computer's GPU can hold` : !held && pair?.estimated_memory_bytes ? "when it is loaded" : undefined}
            >
              {held && budget ? <div className="my-0.5"><Meter share={held / budget} tone={held / budget > 1 ? "warn" : "accent"} /></div> : null}
            </Stat>
            <Stat
              label="Answered since the service started"
              value={`${count(stats.totals.requests)} ${stats.totals.requests === 1 ? "request" : "requests"}`}
              note={stats.totals.requests ? `${count(stats.totals.prompt_tokens)} tokens read, ${count(stats.totals.completion_tokens)} written` : "Point Hermes, kr8 or any OpenAI client at the endpoint"}
            />
          </div>
        )}

        {showTooLong && (
          <Notice
            action={
              bigger && target ? (
                <Button size="sm" onClick={() => act("window", () => api.select({ loader: target.loader, model: target.model, context: bigger, load: true }))} disabled={busy !== ""}>
                  {busy === "window" ? "Reloading" : `Use ${windowLabel(bigger)}`}
                </Button>
              ) : undefined
            }
          >
            {count(tooLong.count)} {tooLong.count === 1 ? "prompt" : "prompts"} did not fit the {count(window_)}-token window.
            {tooLong.largest_tokens ? ` The largest was about ${count(tooLong.largest_tokens)} tokens.` : ""}
          </Notice>
        )}
      </Card>

      {stats.recent.length > 0 && (
        <section className="flex flex-col gap-2">
          <div className="flex items-center justify-between">
            <Label>Latest requests</Label>
            <Button variant="ghost" size="sm" onClick={openRequests}>All requests</Button>
          </div>
          <Card className="divide-y divide-line">
            {stats.recent.slice(0, 5).map((r, i) => (
              <div key={`${r.at}-${i}`} className="flex flex-wrap items-center justify-between gap-x-6 gap-y-1 px-5 py-3">
                <span className="w-20 font-mono text-xs text-muted">{clock(r.at)}</span>
                <span className="min-w-0 flex-1 truncate">{modelName(r.model)} <span className="text-muted">on {nameOf(r.loader)}</span></span>
                <span className="text-muted">{r.prompt_tokens != null ? `${count(r.prompt_tokens)} read, ${count(r.completion_tokens ?? 0)} written` : ""}</span>
                <span className="w-24 text-right text-muted">{r.tokens_per_second ? `${r.tokens_per_second.toFixed(1)} tok/s` : seconds(r.duration_ms)}</span>
                <span className="w-20 text-right">
                  {r.finish === "ok" ? <span className="text-ok">Answered</span> : r.finish === "overflow" ? <span className="text-warn">Too long</span> : r.finish === "cancelled" ? <span className="text-muted">Cancelled</span> : <span className="text-bad">Failed</span>}
                </span>
              </div>
            ))}
          </Card>
        </section>
      )}
    </>
  );
}
