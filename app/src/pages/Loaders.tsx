import { Card, Chip, Notice, PageTitle, State, Button } from "../components/ui";
import { api, Loader } from "../lib/api";
import { Service } from "../lib/useService";
import { requirementNotes } from "./Models";
import { useState } from "react";

function loaderState(l: Loader): { tone: "ok" | "warn" | "off"; text: string } {
  if (!l.supported_here) return { tone: "off", text: "Not for this kind of computer" };
  if (l.kind === "attach") {
    if (l.reachable) return { tone: "ok", text: "Its server is answering" };
    return l.program ? { tone: "warn", text: "Installed; its server is not running" } : { tone: "off", text: "Not installed" };
  }
  return l.installed ? { tone: "ok", text: "Installed" } : { tone: "off", text: "Not installed" };
}

export function Loaders({ svc }: { svc: Service }) {
  const [busy, setBusy] = useState(false);
  const rescan = async () => {
    setBusy(true);
    try {
      await api.rescan();
    } catch {
      // the lists below simply stay as they were
    }
    await svc.refresh();
    setBusy(false);
  };
  return (
    <>
      <PageTitle actions={<Button onClick={rescan} disabled={busy}>{busy ? "Looking" : "Look again"}</Button>}>Loaders</PageTitle>
      <div className="flex flex-col gap-3">
        {svc.loaders.map((l) => {
          const state = loaderState(l);
          const c = l.capabilities;
          const chips = [`Runs ${l.formats.join(", ")}`, c.tools && "Tools", c.json_schema && "JSON schema", c.vision && "Pictures", c.anthropic_api && "Anthropic API", c.sessions > 1 && `Up to ${c.sessions} conversations`].filter(Boolean) as string[];
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
                {l.description} {l.kind === "managed" ? "8gent starts and stops it." : `8gent uses the server ${l.name} runs.`}
              </p>
              <div className="flex flex-wrap items-center gap-2">
                <Chip>{l.models.length === 1 ? "1 model here" : `${l.models.length} models here`}</Chip>
                {chips.map((x) => <Chip key={x}>{x}</Chip>)}
              </div>
              {requirementNotes(l).map((n) => <Notice key={n}>{n}</Notice>)}
            </Card>
          );
        })}
      </div>
    </>
  );
}
