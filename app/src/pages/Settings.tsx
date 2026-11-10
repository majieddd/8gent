import { useState } from "react";
import { CopyButton, Theme } from "../App";
import { Button, Card, cx, Label, Notice, PageTitle, Segmented } from "../components/ui";
import { api, ApiError, Status } from "../lib/api";
import { Service } from "../lib/useService";

function Row({ title, hint, children }: { title: string; hint?: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-wrap items-center justify-between gap-x-8 gap-y-3 px-5 py-4">
      <div className="flex min-w-0 max-w-xl flex-col gap-0.5">
        <span className="font-medium">{title}</span>
        {hint && <span className="text-[13px] text-muted">{hint}</span>}
      </div>
      <div className="flex shrink-0 flex-wrap items-center gap-2.5">{children}</div>
    </div>
  );
}

function Switch({ on, onChange, label }: { on: boolean; onChange: (v: boolean) => void; label: string }) {
  return (
    <button type="button" role="switch" aria-checked={on} aria-label={label} onClick={() => onChange(!on)} className={cx("relative h-5 w-9 rounded-full transition-colors duration-base ease-kr8", on ? "bg-primary" : "bg-track")}>
      <span className={cx("absolute left-0.5 top-0.5 h-4 w-4 rounded-full bg-card transition-transform duration-base ease-kr8", on && "translate-x-4")} />
    </button>
  );
}

export function Settings({ svc, status, theme, setTheme, endpoint }: { svc: Service; status: Status; theme: Theme; setTheme: (t: Theme) => void; endpoint: string }) {
  const [error, setError] = useState("");
  const [stopping, setStopping] = useState(false);
  const save = async (body: Parameters<typeof api.settings>[0]) => {
    setError("");
    try {
      await api.settings(body);
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "The service did not answer.");
    }
    await svc.refresh();
  };
  const stopService = async () => {
    setStopping(true);
    try {
      await api.shutdown();
    } catch {
      // it is gone either way; the status poll notices
    }
    await svc.refresh();
    setStopping(false);
  };

  return (
    <>
      <PageTitle>Settings</PageTitle>
      {error && <Notice tone="bad">{error}</Notice>}

      <section className="flex flex-col gap-2">
        <Label>Memory</Label>
        <Card className="divide-y divide-line">
          <Row title="When to keep the model loaded" hint="A loaded model answers at once but holds its memory. An unloaded one loads again on the next request.">
            <Segmented
              label="When to keep the model loaded"
              value={status.memory_mode}
              onChange={(memory_mode) => save({ memory_mode })}
              options={[{ value: "idle", label: "Unload when idle" }, { value: "on_demand", label: "Keep once loaded" }, { value: "always", label: "Load at start" }]}
            />
          </Row>
          {status.memory_mode === "idle" && (
            <Row title="Unload after" hint="Time without a request before the model is unloaded.">
              <Segmented label="Unload after" value={status.idle_minutes} onChange={(idle_minutes) => save({ idle_minutes })} options={[5, 15, 30, 60].concat([5, 15, 30, 60].includes(status.idle_minutes) ? [] : [status.idle_minutes]).sort((a, b) => a - b).map((m) => ({ value: m, label: `${m} min` }))} />
            </Row>
          )}
        </Card>
      </section>

      <section className="flex flex-col gap-2">
        <Label>Loaders</Label>
        <Card>
          <Row title="Update loaders automatically" hint="Once a day 8gent looks for newer engines and installs them when nothing has been asked for half an hour. The model is unloaded for an update and loaded again after it.">
            <Switch on={status.auto_update} onChange={(auto_update) => save({ auto_update })} label="Update loaders automatically" />
          </Row>
        </Card>
      </section>

      <section className="flex flex-col gap-2">
        <Label>Endpoint</Label>
        <Card className="divide-y divide-line">
          <Row title="Address" hint={`For Hermes, kr8 and any OpenAI client. The model name "${status.served_name}", or any name 8gent does not know, means whatever is selected.`}>
            <span className="font-mono text-[13px]">{endpoint}</span>
            <CopyButton text={endpoint} label="Copy the endpoint address" className="text-muted" />
          </Row>
          {status.also_listening.length > 0 && (
            <Row title="Also answering on" hint="The addresses of the Home apps 8gent replaced, so whatever was pointed at them keeps working.">
              <span className="font-mono text-[13px]">{status.also_listening.map((p) => `:${p}`).join("  ")}</span>
            </Row>
          )}
          <Row title="Requests can switch models" hint="A request that names another model, or loader/model, switches to it. Off: such a request is refused.">
            <Switch on={status.auto_switch} onChange={(auto_switch) => save({ auto_switch })} label="Requests can switch models" />
          </Row>
        </Card>
      </section>

      <section className="flex flex-col gap-2">
        <Label>Appearance</Label>
        <Card>
          <Row title="Theme">
            <Segmented label="Theme" value={theme} onChange={setTheme} options={[{ value: "system", label: "Follow the system" }, { value: "light", label: "Light" }, { value: "dark", label: "Dark" }]} />
          </Row>
        </Card>
      </section>

      <section className="flex flex-col gap-2">
        <Label>Service</Label>
        <Card className="divide-y divide-line">
          <Row title={`8gent ${status.version}`} hint={`Running on ${status.platform} ${status.os_version}. It keeps the endpoint up when this window is closed.`}>
            <Button onClick={stopService} disabled={stopping}>{stopping ? "Stopping" : "Stop the service"}</Button>
          </Row>
          <Row title="Start at login" hint="The endpoint is there as soon as you log in, without opening this window.">
            <Switch
              on={status.login_item}
              label="Start at login"
              onChange={async (enabled) => {
                setError("");
                try {
                  await api.loginItem(enabled);
                } catch (e) {
                  setError(e instanceof ApiError ? e.message : "The service did not answer.");
                }
                // switching it on restarts the service under the login agent: give it a moment
                setTimeout(svc.refresh, 4500);
                await svc.refresh();
              }}
            />
          </Row>
          <Row title="Data folder" hint="Settings, the list of models you add by hand, logs and the request history.">
            <span className="max-w-[420px] truncate font-mono text-xs text-muted" title={status.home}>{status.home}</span>
            <CopyButton text={status.home} label="Copy the data folder's path" className="text-muted" />
          </Row>
        </Card>
      </section>
    </>
  );
}
