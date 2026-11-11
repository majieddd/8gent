import { Check, Copy } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { Button, Card, Notice, PageTitle, State } from "../components/ui";
import { api, ApiError, Harness, HarnessWritten } from "../lib/api";
import { harnessState, hasOwnEntry, linkedThrough } from "../lib/harness";

function CopyLine({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  const copy = () => {
    navigator.clipboard?.writeText(text).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    }, () => {});
  };
  return (
    <div className="flex items-center justify-between gap-3 rounded-md bg-soft px-3 py-2">
      <code className="min-w-0 break-all font-mono text-xs">{text}</code>
      <button type="button" onClick={copy} aria-label={copied ? "Copied" : "Copy this command"} className="flex h-7 w-7 flex-none items-center justify-center rounded-md text-muted hover:text-fg">
        {copied ? <Check size={15} aria-hidden /> : <Copy size={15} aria-hidden />}
      </button>
    </div>
  );
}

export function Harnesses() {
  const [list, setList] = useState<Harness[] | null>(null);
  const [endpoint, setEndpoint] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState("");
  // what the last connect or disconnect did, by harness
  const [done, setDone] = useState<Record<string, { on: boolean; written: HarnessWritten }>>({});
  const [failed, setFailed] = useState<Record<string, string>>({});

  const pull = useCallback(async () => {
    try {
      const r = await api.harnesses();
      setList(r.harnesses);
      setEndpoint(r.endpoint);
      setError("");
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "The service did not answer.");
    }
  }, []);
  useEffect(() => {
    pull();
    // a harness's settings can be changed from its own side at any time
    const t = setInterval(pull, 15000);
    return () => clearInterval(t);
  }, [pull]);

  const change = async (h: Harness, on: boolean) => {
    setBusy(h.id);
    setFailed((f) => ({ ...f, [h.id]: "" }));
    try {
      const r = await api.connectHarness(h.id, on);
      setDone((d) => ({ ...d, [h.id]: { on, written: r.written } }));
      setList((l) => (l ? l.map((x) => (x.id === h.id ? r.harness : x)) : l));
    } catch (e) {
      setFailed((f) => ({ ...f, [h.id]: e instanceof ApiError ? e.message : "The service did not answer." }));
    }
    setBusy("");
  };

  return (
    <>
      <PageTitle actions={<Button onClick={pull}>Look again</Button>}>Harnesses</PageTitle>
      <p className="m-0 max-w-3xl text-muted">
        A harness is the program that turns a model into an agent: it gives it tools, files and a way to work in steps. 8gent can point the ones on this computer at its endpoint{endpoint ? <> (<span className="font-mono text-xs">{endpoint}</span>)</> : ""}. It adds only its own entry to a harness's settings, keeps a copy of the file first, and leaves the harness's default model alone.
      </p>
      {error && <Notice tone="bad">{error}</Notice>}
      {!list && !error && <p className="m-0 text-muted">Looking for harnesses.</p>}
      {list && list.length === 0 && <p className="m-0 text-muted">8gent knows no harnesses.</p>}
      <div className="flex flex-col gap-3">
        {(list ?? []).map((h) => {
          const state = harnessState(h);
          const last = done[h.id];
          const link = h.link;
          return (
            <Card key={h.id} tone={link.state === "connected" && h.installed ? "accent" : undefined} className="flex flex-col gap-3 px-5 py-[18px]">
              <div className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1">
                <div className="flex flex-wrap items-baseline gap-2.5">
                  <span className="text-base font-semibold leading-[22px]">{h.name}</span>
                  {h.version && <span className="font-mono text-xs text-muted">{h.version}</span>}
                </div>
                <State tone={state.tone}>{state.text}</State>
              </div>
              {h.summary && <p className="m-0 max-w-3xl text-muted">{h.summary}</p>}

              {!h.installed && h.homepage && <p className="m-0 text-[13px] text-muted">Where to get it: <span className="font-mono text-xs">{h.homepage}</span></p>}

              {h.installed && link.state === "connected" && (
                <>
                  <p className="m-0">{linkedThrough(h)}</p>
                  {h.use && hasOwnEntry(h) && <CopyLine text={h.use} />}
                  {h.settings_file && <p className="m-0 break-all font-mono text-xs text-muted">{h.settings_file}</p>}
                  {hasOwnEntry(h) && (
                    <div>
                      <Button size="sm" onClick={() => change(h, false)} disabled={busy !== ""}>{busy === h.id ? "Working" : "Disconnect"}</Button>
                    </div>
                  )}
                </>
              )}

              {h.installed && link.state === "not_connected" && h.can_connect && (
                <Notice action={<Button size="sm" variant="primary" onClick={() => change(h, true)} disabled={busy !== ""}>{busy === h.id ? "Working" : "Connect"}</Button>}>
                  8gent adds itself to {h.name}'s settings ({h.settings_file}). The file as it is now is copied first, and {h.name}'s default model is not changed.
                </Notice>
              )}

              {h.installed && link.state === "unreadable" && (
                <Notice>
                  {h.name}: {link.why}. 8gent will not change that file. To connect by hand, add a provider that speaks the OpenAI API at {endpoint}.
                </Notice>
              )}

              {h.installed && link.state === "not_by_eightgent" && link.why && <p className="m-0 max-w-3xl">{link.why}</p>}

              {failed[h.id] && <Notice tone="bad">{failed[h.id]}</Notice>}
              {last && !failed[h.id] && (
                <p className="m-0 text-[13px] text-muted">
                  {last.written.changed ? (last.on ? `Connected: ${last.written.file} was changed.` : `Disconnected: 8gent's entry was taken out of ${last.written.file}.`) : "Nothing needed changing."}
                  {last.written.backup ? ` The file as it was: ${last.written.backup}` : ""}
                </p>
              )}
            </Card>
          );
        })}
      </div>
    </>
  );
}
