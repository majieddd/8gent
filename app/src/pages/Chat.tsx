// 8gent's own conversations with whatever is selected. Only what is said here is kept here; what
// other programs ask through the endpoint shows in Requests. For now a conversation goes straight
// to the model; tools and previews arrive with the harness layer.
import { ArrowUp, Brain, Check, Copy, Paperclip, Plus, Square, Trash2, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { Button, cx, Mark8, Notice } from "../components/ui";
import { api, ApiError, ChatDoc, ChatMessage, ChatRow, Status, streamChat } from "../lib/api";
import { seconds } from "../lib/format";
import { Service } from "../lib/useService";

interface Props {
  svc: Service;
  status: Status;
  modelName: (id: string) => string;
  loaderName: (id: string) => string;
  openModels: () => void;
}

const blank = (): ChatDoc => ({ id: crypto.randomUUID(), title: "New chat", messages: [] });

function CodeBlock({ children }: { children?: React.ReactNode }) {
  const box = useRef<HTMLPreElement>(null);
  const [done, setDone] = useState(false);
  const copy = () => navigator.clipboard.writeText(box.current?.innerText ?? "").then(() => { setDone(true); setTimeout(() => setDone(false), 1500); }, () => {});
  return (
    <div className="relative my-2.5">
      <button type="button" aria-label="Copy the code" onClick={copy} className="absolute right-1.5 top-1.5 inline-flex h-7 w-7 items-center justify-center rounded-sm bg-card text-muted hover:text-fg">
        {done ? <Check aria-hidden className="h-[15px] w-[15px]" strokeWidth={1.75} /> : <Copy aria-hidden className="h-[15px] w-[15px]" strokeWidth={1.75} />}
      </button>
      <pre ref={box} tabIndex={0} className="m-0 overflow-x-auto rounded-lg border border-line bg-soft px-4 py-3 font-mono text-[13px] leading-5">{children}</pre>
    </div>
  );
}

const MARKDOWN = {
  p: (p: any) => <p className="my-2 first:mt-0 last:mb-0" {...p} />,
  h1: (p: any) => <h3 className="mb-2 mt-4 text-lg font-semibold first:mt-0" {...p} />,
  h2: (p: any) => <h3 className="mb-2 mt-4 text-base font-semibold first:mt-0" {...p} />,
  h3: (p: any) => <h4 className="mb-1.5 mt-3 font-semibold first:mt-0" {...p} />,
  ul: (p: any) => <ul className="my-2 list-disc space-y-1 pl-5" {...p} />,
  ol: (p: any) => <ol className="my-2 list-decimal space-y-1 pl-5" {...p} />,
  a: (p: any) => <a className="text-accent underline underline-offset-2" target="_blank" rel="noreferrer" {...p} />,
  blockquote: (p: any) => <blockquote className="my-2 border-l-2 border-line pl-3 text-muted" {...p} />,
  pre: (p: any) => <CodeBlock>{p.children}</CodeBlock>,
  code: ({ className, children, ...rest }: any) => (className || String(children).includes("\n") ? <code className={className} {...rest}>{children}</code> : <code className="rounded-sm bg-soft px-1 py-0.5 font-mono text-[13px]" {...rest}>{children}</code>),
  table: (p: any) => <div className="my-2.5 overflow-x-auto"><table className="border-collapse text-left" {...p} /></div>,
  th: (p: any) => <th className="border border-line bg-soft px-3 py-1.5 font-medium" {...p} />,
  td: (p: any) => <td className="border border-line px-3 py-1.5" {...p} />,
};

function Answer({ m, live }: { m: ChatMessage; live: boolean }) {
  const stats = [m.tokens != null && `${m.tokens.toLocaleString("en-US")} tokens`, m.tokens_per_second && `${m.tokens_per_second.toFixed(1)} tokens/s`, m.first_token_ms != null && `first token after ${seconds(m.first_token_ms)}`, m.by].filter(Boolean);
  return (
    <div className="flex min-w-0 flex-col gap-1.5">
      {m.thinking && (
        <details className="group rounded-lg border border-line px-3 py-2 text-muted" open={live && !m.content}>
          <summary className="cursor-pointer select-none text-[13px] font-medium">{live && !m.content ? "Thinking" : `Thought for ${seconds(m.thinking_ms ?? 0)}`}</summary>
          <div className="mt-2 whitespace-pre-wrap text-[13px] leading-5">{m.thinking}</div>
        </details>
      )}
      {m.content && <div className="min-w-0 break-words leading-6"><ReactMarkdown remarkPlugins={[remarkGfm]} components={MARKDOWN}>{m.content}</ReactMarkdown></div>}
      {live && !m.content && !m.thinking && <span className="text-muted">Waiting for the first words</span>}
      {m.error && <Notice tone="bad">{m.error}</Notice>}
      {!live && (stats.length > 0 || m.stopped) && <div className="text-xs text-muted">{[m.stopped && "Stopped", ...stats].filter(Boolean).join(" · ")}</div>}
    </div>
  );
}

export function Chat({ svc, status, modelName, loaderName, openModels }: Props) {
  const [rows, setRows] = useState<ChatRow[]>([]);
  const [chat, setChat] = useState<ChatDoc>(blank);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [thinking, setThinking] = useState(status.selection.thinking);
  const [file, setFile] = useState<{ name: string; text: string } | null>(null);
  const [error, setError] = useState("");
  const abort = useRef<AbortController | null>(null);
  const scroller = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const picker = useRef<HTMLInputElement>(null);

  const target = status.engine.running ?? (status.selection.model ? status.selection : null);
  const who = target ? `${modelName(target.model)} on ${loaderName(target.loader)}` : "";

  const pullRows = useCallback(() => api.chats().then((r) => setRows(r.chats), () => {}), []);
  useEffect(() => {
    pullRows();
  }, [pullRows]);
  useEffect(() => {
    if (stick.current && scroller.current) scroller.current.scrollTop = scroller.current.scrollHeight;
  }, [chat]);
  useEffect(() => () => abort.current?.abort(), []);

  const open = async (id: string) => {
    if (busy) return;
    try {
      setChat(await api.chat(id));
      stick.current = true;
      setError("");
    } catch {
      setError("That conversation could not be opened.");
    }
  };

  const remove = async (id: string) => {
    await api.deleteChat(id).catch(() => {});
    if (id === chat.id) setChat(blank());
    pullRows();
  };

  const attach = async (f: File | undefined) => {
    if (!f) return;
    if (f.size > 2_000_000) return setError(`${f.name} is larger than 2 MB. Attach a smaller text file.`);
    const text = await f.text();
    if (text.includes("\u0000")) return setError(`${f.name} does not look like a text file.`);
    setFile({ name: f.name, text });
    setError("");
  };

  const send = async () => {
    const text = draft.trim();
    if ((!text && !file) || busy || !target) return;
    const content = file ? `${text}${text ? "\n\n" : ""}File ${file.name}:\n\`\`\`\n${file.text}\n\`\`\`` : text;
    const history: ChatMessage[] = [...chat.messages, { role: "user", content }];
    const answer: ChatMessage = { role: "assistant", content: "", by: who };
    const title = chat.messages.length === 0 ? (text || file?.name || "New chat").replace(/\s+/g, " ").slice(0, 60) : chat.title;
    setChat({ ...chat, title, messages: [...history, answer] });
    setDraft("");
    setFile(null);
    setError("");
    setBusy(true);
    stick.current = true;

    const controller = new AbortController();
    abort.current = controller;
    const began = performance.now();
    let first = 0;
    let thoughtEnd = 0;
    let frame = 0;
    // the answer grows in `answer`; the screen follows once a frame, however fast the tokens come
    const show = () => {
      if (frame) return;
      frame = requestAnimationFrame(() => {
        frame = 0;
        setChat((c) => ({ ...c, messages: [...history, { ...answer }] }));
      });
    };
    try {
      await streamChat(
        { model: status.served_name, stream: true, stream_options: { include_usage: true }, enable_thinking: thinking, messages: history.map(({ role, content }) => ({ role, content })) },
        (ev) => {
          if (ev.error) {
            answer.error = ev.error.message ?? String(ev.error);
            return show();
          }
          const delta = ev.choices?.[0]?.delta;
          const thought = delta?.reasoning_content ?? delta?.reasoning;
          if (thought) {
            if (!first) first = performance.now();
            answer.thinking = (answer.thinking ?? "") + thought;
          }
          if (delta?.content) {
            if (!first) first = performance.now();
            if (answer.thinking && !thoughtEnd) thoughtEnd = performance.now();
            answer.content += delta.content;
          }
          if (ev.usage?.completion_tokens != null) answer.tokens = ev.usage.completion_tokens;
          show();
        },
        controller.signal,
      );
    } catch (e) {
      answer.error = e instanceof ApiError ? e.message : "The service did not answer.";
    }
    const ended = performance.now();
    if (controller.signal.aborted) answer.stopped = true;
    if (first) {
      answer.first_token_ms = Math.round(first - began);
      if (answer.thinking) answer.thinking_ms = Math.round((thoughtEnd || ended) - first);
      const writing = (ended - first) / 1000;
      if (answer.tokens && answer.tokens > 1 && writing > 0.05) answer.tokens_per_second = (answer.tokens - 1) / writing;
    }
    if (frame) cancelAnimationFrame(frame);
    const done: ChatDoc = { id: chat.id, title, messages: [...history, { ...answer }] };
    setChat(done);
    setBusy(false);
    abort.current = null;
    await api.saveChat(done).catch(() => setError("This conversation could not be saved."));
    pullRows();
    svc.refresh();
  };

  return (
    <div className="flex min-h-0 flex-1 gap-6">
      <aside aria-label="Conversations" className="hidden w-[220px] shrink-0 flex-col gap-2 md:flex">
        <Button onClick={() => !busy && setChat(blank())} disabled={busy} className="justify-start">
          <Plus aria-hidden className="h-4 w-4" strokeWidth={1.75} />
          New chat
        </Button>
        <div className="flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto">
          {rows.map((r) => (
            <div key={r.id} className={cx("group flex items-center rounded-md", r.id === chat.id ? "bg-soft" : "hover:bg-soft")}>
              <button type="button" onClick={() => open(r.id)} aria-current={r.id === chat.id ? "true" : undefined} className="min-w-0 flex-1 truncate px-2.5 py-2 text-left">
                {r.title}
              </button>
              <button type="button" aria-label={`Delete "${r.title}"`} onClick={() => remove(r.id)} className="mr-1 inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-sm text-muted opacity-0 hover:text-bad focus-visible:opacity-100 group-hover:opacity-100">
                <Trash2 aria-hidden className="h-[15px] w-[15px]" strokeWidth={1.75} />
              </button>
            </div>
          ))}
          {rows.length === 0 && <p className="m-0 px-2.5 py-2 text-[13px] text-muted">Conversations you have here are kept in this list.</p>}
        </div>
      </aside>

      <section aria-label="Conversation" className="flex min-w-0 flex-1 flex-col">
        <div
          ref={scroller}
          onScroll={(e) => {
            const el = e.currentTarget;
            stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
          }}
          className="flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto pb-4 pr-1"
        >
          {chat.messages.length === 0 && (
            <div className="m-auto flex max-w-md flex-col items-center gap-3 text-center">
              <Mark8 className="h-10 w-6" />
              <h1 className="m-0 text-xl font-semibold tracking-[-0.01em]">{target ? `Ask ${modelName(target.model)}` : "Nothing is selected yet"}</h1>
              <p className="m-0 text-muted">What you say here stays in 8gent. What other apps ask through the endpoint shows in Requests, not here.</p>
              {!target && <Button variant="primary" onClick={openModels}>Choose a model</Button>}
            </div>
          )}
          {chat.messages.map((m, i) =>
            m.role === "user" ? (
              <div key={i} className="max-w-[85%] self-end whitespace-pre-wrap break-words rounded-xl bg-soft px-4 py-2.5 leading-6">{m.content}</div>
            ) : (
              <Answer key={i} m={m} live={busy && i === chat.messages.length - 1} />
            ),
          )}
        </div>

        {error && <div className="pb-2"><Notice tone="bad">{error}</Notice></div>}

        <div className="flex flex-col gap-2 rounded-xl border border-line bg-card p-2.5 focus-within:border-accent">
          {file && (
            <span className="inline-flex h-7 w-fit items-center gap-1.5 rounded-full border border-line pl-3 pr-1 text-[13px]">
              {file.name}
              <button type="button" aria-label={`Remove ${file.name}`} onClick={() => setFile(null)} className="inline-flex h-5 w-5 items-center justify-center rounded-full text-muted hover:text-fg">
                <X aria-hidden className="h-3.5 w-3.5" strokeWidth={1.75} />
              </button>
            </span>
          )}
          <label className="sr-only" htmlFor="chat-message">Message</label>
          <textarea
            id="chat-message"
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                e.preventDefault();
                send();
              }
            }}
            rows={Math.min(8, Math.max(1, draft.split("\n").length))}
            placeholder={target ? `Message ${modelName(target.model)}` : "Choose a model first"}
            disabled={!target}
            className="max-h-52 w-full resize-none border-0 bg-transparent px-1.5 py-1 leading-6 text-fg outline-none placeholder:text-muted"
          />
          <div className="flex items-center justify-between gap-2">
            <div className="flex items-center gap-1.5">
              <input ref={picker} type="file" className="hidden" accept="text/*,.md,.json,.csv,.log,.py,.rs,.ts,.tsx,.js,.toml,.yaml,.yml,.swift,.c,.cpp,.h,.go,.java,.sh" onChange={(e) => { attach(e.target.files?.[0]); e.target.value = ""; }} />
              <Button variant="ghost" size="sm" onClick={() => picker.current?.click()} disabled={!target || busy}>
                <Paperclip aria-hidden className="h-4 w-4" strokeWidth={1.75} />
                Attach a text file
              </Button>
              <Button variant="ghost" size="sm" aria-pressed={thinking} onClick={() => setThinking(!thinking)} className={thinking ? "text-accent" : undefined}>
                <Brain aria-hidden className="h-4 w-4" strokeWidth={1.75} />
                {thinking ? "Thinking on" : "Thinking off"}
              </Button>
            </div>
            <div className="flex items-center gap-2.5">
              <span className="hidden truncate text-xs text-muted sm:inline">{who}</span>
              {busy ? (
                <Button variant="primary" size="sm" onClick={() => abort.current?.abort()} aria-label="Stop the answer" className="w-8 px-0">
                  <Square aria-hidden className="h-3.5 w-3.5 fill-current" strokeWidth={1.75} />
                </Button>
              ) : (
                <Button variant="primary" size="sm" onClick={send} disabled={!target || (!draft.trim() && !file)} aria-label="Send" className="w-8 px-0">
                  <ArrowUp aria-hidden className="h-4 w-4" strokeWidth={2} />
                </Button>
              )}
            </div>
          </div>
        </div>
      </section>
    </div>
  );
}
