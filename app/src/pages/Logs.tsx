import { useEffect, useRef, useState } from "react";
import { CopyButton } from "../App";
import { Card, PageTitle, Segmented } from "../components/ui";
import { api, Status } from "../lib/api";
import { Service } from "../lib/useService";

/** The engine's own output, refreshed every 2 s, kept at the newest line unless you scroll up. */
export function Logs({ svc, status }: { svc: Service; status: Status }) {
  const managed = svc.loaders.filter((l) => l.kind === "managed" && l.installed);
  const first = managed.find((l) => l.id === (status.engine.running?.loader ?? status.selection.loader))?.id ?? managed[0]?.id ?? "";
  const [loader, setLoader] = useState(first);
  const [lines, setLines] = useState<string[]>([]);
  const box = useRef<HTMLPreElement>(null);
  const stick = useRef(true);

  useEffect(() => {
    if (!loader) return;
    let alive = true;
    const pull = () => api.logs(loader).then((r) => alive && setLines(r.lines), () => {});
    setLines([]);
    stick.current = true;
    pull();
    const t = setInterval(pull, 2000);
    return () => {
      alive = false;
      clearInterval(t);
    };
  }, [loader]);

  useEffect(() => {
    if (stick.current && box.current) box.current.scrollTop = box.current.scrollHeight;
  }, [lines]);

  return (
    <>
      <PageTitle actions={managed.length > 1 ? <Segmented label="Loader" value={loader} onChange={setLoader} options={managed.map((l) => ({ value: l.id, label: l.name }))} /> : undefined}>Logs</PageTitle>
      {!loader ? (
        <p className="m-0 text-muted">No loader that 8gent starts itself is installed, so there is no engine output to show.</p>
      ) : lines.length === 0 ? (
        <p className="m-0 text-muted">This loader has not written anything yet. Its output appears here once it has loaded a model.</p>
      ) : (
        <Card className="relative">
          <CopyButton text={lines.join("\n")} label="Copy the log" className="absolute right-2 top-2 bg-card text-muted" />
          <pre
            ref={box}
            tabIndex={0}
            onScroll={(e) => {
              const el = e.currentTarget;
              stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
            }}
            className="m-0 max-h-[calc(100vh-190px)] min-h-[240px] overflow-auto whitespace-pre-wrap break-words px-5 py-4 font-mono text-xs leading-[18px]"
          >
            {lines.join("\n")}
          </pre>
        </Card>
      )}
    </>
  );
}
