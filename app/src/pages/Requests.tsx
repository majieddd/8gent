import { useEffect, useState } from "react";
import { Card, PageTitle } from "../components/ui";
import { api, RequestRecord } from "../lib/api";
import { clock, count, seconds } from "../lib/format";

const RESULT: Record<string, [string, string]> = { ok: ["Answered", "text-ok"], overflow: ["Too long", "text-warn"], cancelled: ["Cancelled", "text-muted"], error: ["Failed", "text-bad"] };

export function Requests({ modelName, loaderName }: { modelName: (id: string) => string; loaderName: (id: string) => string }) {
  const [rows, setRows] = useState<RequestRecord[] | null>(null);
  useEffect(() => {
    let alive = true;
    const pull = () => api.requests().then((r) => alive && setRows(r.requests), () => {});
    pull();
    const t = setInterval(pull, 2000);
    return () => {
      alive = false;
      clearInterval(t);
    };
  }, []);

  return (
    <>
      <PageTitle>Requests</PageTitle>
      {rows && rows.length === 0 && <p className="m-0 text-muted">Nothing has been asked since the service started.</p>}
      {rows && rows.length > 0 && (
        <Card className="overflow-x-auto">
          <table className="w-full min-w-[760px] border-collapse text-left">
            <thead>
              <tr className="text-xs text-muted">
                {["Time", "Model", "Loader", "Read", "Written", "First token", "Speed", "Took", "Result"].map((h, i) => (
                  <th key={h} scope="col" className={`px-4 py-2.5 font-medium ${i >= 3 && i <= 7 ? "text-right" : ""}`}>{h}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {rows.map((r, i) => {
                const [word, tone] = RESULT[r.finish] ?? RESULT.error;
                return (
                  <tr key={`${r.at}-${i}`} className="border-t border-line">
                    <td className="px-4 py-2.5 font-mono text-xs text-muted">{clock(r.at)}</td>
                    <td className="max-w-[220px] truncate px-4 py-2.5">{modelName(r.model)}</td>
                    <td className="px-4 py-2.5 text-muted">{loaderName(r.loader)}{r.stream ? "" : " · whole answer"}</td>
                    <td className="px-4 py-2.5 text-right tabular-nums">{count(r.prompt_tokens)}</td>
                    <td className="px-4 py-2.5 text-right tabular-nums">{count(r.completion_tokens)}</td>
                    <td className="px-4 py-2.5 text-right tabular-nums text-muted">{seconds(r.first_token_ms)}</td>
                    <td className="px-4 py-2.5 text-right tabular-nums">{r.tokens_per_second ? `${r.tokens_per_second.toFixed(1)} tok/s` : ""}</td>
                    <td className="px-4 py-2.5 text-right tabular-nums text-muted">{seconds(r.duration_ms)}</td>
                    <td className={`px-4 py-2.5 ${tone}`}>{word}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </Card>
      )}
    </>
  );
}
