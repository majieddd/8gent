import { useEffect, useState } from "react";
import { Card, Label, PageTitle, State } from "../components/ui";
import { api, Sample, Status } from "../lib/api";
import { count, gb } from "../lib/format";

/** The last two minutes of one number as a line. The words beside it carry the values. */
function Sparkline({ values, ceiling, label }: { values: number[]; ceiling: number; label: string }) {
  const [w, h] = [600, 72];
  const top = Math.max(ceiling, ...values, 1e-9);
  const points = values.map((v, i) => `${(values.length === 1 ? w : (i / (values.length - 1)) * w).toFixed(1)},${(h - 2 - (v / top) * (h - 6)).toFixed(1)}`);
  return (
    <svg viewBox={`0 0 ${w} ${h}`} preserveAspectRatio="none" role="img" aria-label={label} className="h-[72px] w-full">
      {points.length > 1 && (
        <>
          <polygon points={`0,${h} ${points.join(" ")} ${w},${h}`} fill="var(--accent)" opacity="0.12" />
          <polyline points={points.join(" ")} fill="none" stroke="var(--accent)" strokeWidth="1.5" vectorEffect="non-scaling-stroke" strokeLinejoin="round" />
        </>
      )}
    </svg>
  );
}

export function Monitor({ status, modelName, loaderName }: { status: Status; modelName: (id: string) => string; loaderName: (id: string) => string }) {
  const [samples, setSamples] = useState<Sample[]>([]);
  const [budget, setBudget] = useState<number | null>(null);
  useEffect(() => {
    let alive = true;
    const pull = () => api.monitor().then((m) => { if (alive) { setSamples(m.samples.slice(-120)); setBudget(m.gpu_budget_bytes); } }, () => {});
    pull();
    const t = setInterval(pull, 1000);
    return () => {
      alive = false;
      clearInterval(t);
    };
  }, []);

  const now = samples[samples.length - 1];
  const run = status.engine.running;
  const speeds = samples.map((s) => s.tokens_per_second);
  const memory = samples.map((s) => s.memory_bytes ?? 0);
  const peak = Math.max(0, ...speeds);
  const writing = (now?.tokens_per_second ?? 0) > 0;
  const span = samples.length > 1 ? `the last ${samples.length >= 120 ? "2 minutes" : `${samples.length} seconds`}` : "now";

  return (
    <>
      <PageTitle>Monitor</PageTitle>

      <Card className="flex flex-wrap items-center justify-between gap-x-8 gap-y-2 px-5 py-4">
        <div className="flex min-w-0 flex-col gap-0.5">
          <span className="truncate text-base font-semibold">{run ? `${modelName(run.model)} on ${loaderName(run.loader)}` : status.selection.model ? `${modelName(status.selection.model)} on ${loaderName(status.selection.loader)}` : "Nothing selected"}</span>
          <span className="text-[13px] text-muted">{run ? `${run.managed ? `${count(run.context)}-token window · ` : ""}${status.stats.in_flight} ${status.stats.in_flight === 1 ? "request" : "requests"} in progress` : "No model is loaded"}</span>
        </div>
        {status.engine.state === "ready" && <State tone={writing ? "accent" : "ok"}>{writing ? "Writing an answer" : status.stats.in_flight > 0 ? "Reading the prompt" : "Ready"}</State>}
        {status.engine.state === "loading" && <State tone="warn">Loading</State>}
        {status.engine.state === "stopped" && <State tone="off">Not loaded</State>}
        {status.engine.state === "error" && <State tone="bad">Problem</State>}
      </Card>

      <div className="grid gap-4 [grid-template-columns:repeat(auto-fit,minmax(min(420px,100%),1fr))]">
        <Card className="flex flex-col gap-3 px-5 py-4">
          <Label>Speed</Label>
          <div className="flex items-baseline justify-between gap-4">
            <span className="text-2xl font-semibold leading-8">{writing ? `${now.tokens_per_second.toFixed(0)} tokens/s` : "Idle"}</span>
            <span className="text-[13px] text-muted">{peak > 0 ? `Highest in ${span}: ${peak.toFixed(0)} tokens/s` : `Nothing written in ${span}`}</span>
          </div>
          <Sparkline values={speeds} ceiling={10} label={`Tokens written per second over ${span}; highest ${peak.toFixed(0)}`} />
        </Card>

        <Card className="flex flex-col gap-3 px-5 py-4">
          <Label>Memory the engine holds</Label>
          <div className="flex items-baseline justify-between gap-4">
            <span className="text-2xl font-semibold leading-8">{now?.memory_bytes ? gb(now.memory_bytes) : run && !run.managed ? `Held by ${loaderName(run.loader)}` : "None"}</span>
            <span className="text-[13px] text-muted">{budget ? `This computer's GPU can hold ${gb(budget)}` : ""}</span>
          </div>
          <Sparkline values={memory} ceiling={budget ?? 1} label={`Memory held by the engine over ${span}${now?.memory_bytes ? `; now ${gb(now.memory_bytes)}` : ""}`} />
        </Card>
      </div>
    </>
  );
}
