import { useCallback, useEffect, useState } from "react";
import { Button, Card, Chip, Notice, PageTitle, State } from "../components/ui";
import { api, ApiError, Loader, Status, UpdateState } from "../lib/api";
import { Service } from "../lib/useService";
import { requirementNotes } from "./Models";

const FORMAT_NAMES: Record<string, string> = { gguf: "GGUF files", mlx: "MLX builds", uzu: "folders converted for uzu", mirai: "Mirai's builds", ollama: "what Ollama has pulled", lmstudio: "what LM Studio has loaded", strata: "Strata's packs" };

function loaderState(l: Loader): { tone: "ok" | "warn" | "off"; text: string } {
  if (!l.supported_here) return { tone: "off", text: "Not for this kind of computer" };
  if (l.kind === "attach") {
    if (l.reachable) return { tone: "ok", text: "Its server is answering" };
    return l.program ? { tone: "warn", text: "Installed; its server is not running" } : { tone: "off", text: "Not installed" };
  }
  return l.installed ? { tone: "ok", text: "Installed" } : { tone: "off", text: "Not installed" };
}

export function Loaders({ svc, status }: { svc: Service; status: Status }) {
  const [updates, setUpdates] = useState<Record<string, UpdateState>>({});
  const [checking, setChecking] = useState(false);
  const [error, setError] = useState("");
  const anyBusy = Object.values(updates).some((u) => u.busy);

  const take = (list: UpdateState[]) => setUpdates(Object.fromEntries(list.map((u) => [u.loader, u])));
  const pull = useCallback(() => api.updates().then((r) => take(r.updates), () => {}), []);
  useEffect(() => {
    pull();
    // while an update runs, follow it; otherwise a glance now and then is enough
    const t = setInterval(pull, anyBusy ? 2000 : 15000);
    return () => clearInterval(t);
  }, [pull, anyBusy]);

  const check = async () => {
    setChecking(true);
    setError("");
    try {
      await api.rescan();
      take((await api.checkUpdates()).updates);
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "The service did not answer.");
    }
    await svc.refresh();
    setChecking(false);
  };

  const update = async (id: string) => {
    setError("");
    try {
      await api.updateLoader(id);
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "The service did not answer.");
    }
    pull();
  };

  const serving = status.engine.running?.loader;

  return (
    <>
      <PageTitle actions={<Button onClick={check} disabled={checking || anyBusy}>{checking ? "Looking" : "Look for updates"}</Button>}>Loaders</PageTitle>
      {error && <Notice tone="bad">{error}</Notice>}
      <div className="flex flex-col gap-3">
        {svc.loaders.map((l) => {
          const state = loaderState(l);
          const c = l.capabilities;
          const u = updates[l.id];
          const chips = [`Runs ${l.formats.map((f) => FORMAT_NAMES[f] ?? f).join(" and ")}`, c.tools && "Tools", c.json_schema && "JSON schema", c.vision && "Pictures", c.anthropic_api && "Anthropic API", c.sessions > 1 && `Up to ${c.sessions} conversations`].filter(Boolean) as string[];
          const can = l.supported_here && l.kind === "managed";
          return (
            <Card key={l.id} tone={l.selected ? "accent" : undefined} className="flex flex-col gap-3 px-5 py-[18px]">
              <div className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1">
                <div className="flex flex-wrap items-baseline gap-2.5">
                  <span className="text-base font-semibold leading-[22px]">{l.name}</span>
                  {l.version && <span className="font-mono text-xs text-muted">{l.version}</span>}
                </div>
                <State tone={state.tone}>{state.text}</State>
              </div>
              <p className="m-0 max-w-3xl text-muted">
                {l.description}{l.kind === "managed" ? " 8gent starts and stops it." : ""}
              </p>
              <div className="flex flex-wrap items-center gap-2">
                <Chip>{l.models.length === 1 ? "1 model here" : `${l.models.length} models here`}</Chip>
                {chips.map((x) => <Chip key={x}>{x}</Chip>)}
              </div>

              {u?.busy && <Notice>Updating {l.name}. {serving === l.id ? "The model is unloaded meanwhile and loaded again afterwards." : ""}</Notice>}
              {can && u && !u.busy && u.status === "available" && (
                <Notice action={<Button size="sm" variant="primary" onClick={() => update(l.id)} disabled={anyBusy}>Update</Button>}>
                  A newer {l.name} is available: {u.latest}{u.current ? ` (you have ${u.current})` : ""}.{serving === l.id ? " The model is unloaded for the update and loaded again after it." : ""}
                </Notice>
              )}
              {can && u && !u.busy && u.status === "not_installed" && (
                <Notice action={<Button size="sm" variant="primary" onClick={() => update(l.id)} disabled={anyBusy}>Install</Button>}>
                  {l.name} is not installed. 8gent can install {u.latest ?? "it"}.
                </Notice>
              )}
              {u && !u.busy && u.status === "held" && <Notice>{l.name} {u.latest} is out. {u.note}</Notice>}
              {u && u.status === "unknown" && u.note && <p className="m-0 text-[13px] text-muted">{u.note}</p>}
              {!(u && u.status === "held") && requirementNotes(l).map((n) => <Notice key={n}>{n}</Notice>)}
              {u?.last_result && !u.busy && <p className="m-0 text-[13px] text-muted">Last update: {u.last_result}</p>}
              {can && u && !u.busy && u.status === "current" && !u.last_result && <p className="m-0 text-[13px] text-muted">Up to date.</p>}
            </Card>
          );
        })}
      </div>
    </>
  );
}
