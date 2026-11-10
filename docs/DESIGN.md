# 8gent design

8gent puts one local endpoint in front of any model loader. You choose a loader (uzu, TensorFold,
llama.cpp, Ollama, LM Studio, Strata) and a model; every app that talks to 8gent keeps the same
address whatever is chosen. It grows out of uzu Home, TensorFold Home and StrataHome, which were
three copies of one idea with a different engine inside each.

Status of each part is marked **built**, **partly built** or **planned**. "Built" means it is in
this repository and covered by the tests described under [Testing](#testing).

## Decisions

| Question | Decision |
|---|---|
| Platforms | macOS, Windows and Linux. Loaders are per platform; 8gent itself is not. |
| Core | Rust: one small binary, no Python or other runtime to install. |
| App | Tauri with React, built from kr8's design system, so its screens can move into kr8. |
| Scope | Model engines first. Agent harnesses (Hermes and others) are added on the same pattern later. |
| First loaders | uzu, TensorFold, llama.cpp managed by 8gent; Ollama, LM Studio and Strata attached. |
| Brand | Part of the kr8 family: lowercase name, kr8's neutral palette, the rainbow "8". |
| Licence | MIT. |
| Home apps | uzu Home and TensorFold Home stop development and point here; 8gent imports their setup. |
| Daily setup | 8gent starts at login; the Home apps no longer start their endpoints. 8gent also answers on their old ports (8585, 8484), and Hermes is pointed at 8gent's own address. |
| Harnesses | Three levels, for Hermes, kr8's agent, opencode and Claude Code: configure them to use 8gent, run them, and serve a whole agent as an endpoint. |
| Chat | 8gent's own Chat screen, with an agent's abilities and previews once the harness layer exists. It holds only the conversations held in it; requests from other harnesses show in Requests, not in Chat. No Open WebUI. |
| Signing | Unsigned for now, like the Home apps. |
| Releases | GitHub releases on this repository, and a page on the kr8 site. |
| Other systems | Built by GitHub for Windows, Linux and macOS; tested on Windows first. |

## Shape

```
  Hermes, kr8, any OpenAI client          8gent app, `8gent` CLI, kr8 settings
              |  /v1                                   |  /8gent/v1
              v                                        v
  +-------------------------------- 8gent service ---------------------------------+
  |  endpoint: wake, route, shape requests, standard errors, measure                |
  |  selection (loader + model + context) . model library . loader definitions      |
  +-----------+----------------------+-----------------------+----------------------+
              | starts and stops     | starts and stops      | uses its address
              v                      v                       v
        llama-server           uzu / tensorfold       Ollama, LM Studio, Strata
```

The service is the product. The app, the command line and kr8 are clients of its control API, so
anything one of them can do, the others can too. **Built:** the service and the command line.
**Partly built:** the app.

## Loaders

A loader is a TOML file in `loaders/` (built in) or `<data folder>/loaders/` (added or replaced by
the user). Adding a loader is adding a file, then passing the conformance tests. **Built.**

```toml
id = "uzu"
name = "uzu"
kind = "managed"                 # 8gent starts and stops it; "attach" uses another app's server
platforms = ["macos-arm64"]      # empty: everywhere
formats = ["uzu", "mirai"]       # the model formats it runs

[detect]                         # where the program is, and how to read its version
paths = ["{data}/engines/uzu/uzu", "~/Library/Application Support/UzuHome/backend/engine/uzu"]
version_file = "VERSION"

[start]                          # the start line; {model} {port} {context} {sessions} {model_id}
args = ["server", "--model", "{model}", "--port", "{port}", "--context-length", "{context}", "--sessions", "{sessions}"]
ready_path = "/v1/models"

[[start.env_when]]               # conditions: thinking, !thinking, macos<26.4, sessions>1, context>=65536
if = "macos<26.4"
set = { UZU_DISABLE_MXU = "1" }

[api]                            # how its API differs from the plain OpenAI one
thinking = "enable_thinking"     # none | enable_thinking | chat_template_kwargs | reasoning_effort
role_normalize = true            # repair a history that resumes with the assistant
overflow_markers = ["context overflow", "context length"]

[capabilities]
tools = true
sessions = 2
reserves_context = true

[[requires]]                     # "from this version on, it needs at least this OS version"
os = "macos"
min = "26.4"
since = "0.6.17"
```

| Loader | Kind | Runs | State |
|---|---|---|---|
| llama.cpp | managed | GGUF | built, tested with a real model |
| TensorFold | managed | MLX; variant `tf06` for the 0.6.6 Python line | built, native engine tested with a real model |
| uzu | managed | converted folders, Mirai's builds | built, tested with a real 9B model |
| Ollama | attach | what Ollama has pulled | built, tested with a real model |
| LM Studio | attach | what LM Studio has loaded | built; not tested with its server running |
| Strata | attach | Strata's packs | definition written; Strata is not installed on the development Mac, and its packs need 32 GB or more |

### Updating and installing engines

**Built** for two kinds of loader, named in a loader's `[update]`:

- `brew`: the engine is a Homebrew formula (llama.cpp, TensorFold). Homebrew says whether it is
  outdated (its own judgement: llama.cpp's version went from 8680 to 0.9.0) and upgrades or installs it.
- `github-prebuilt`: versions are one repository's tags, ready-made engines another's release files
  (uzu: Mirai's tags, builds with uzu Home's patches). A download must match the SHA-256 GitHub
  publishes for it, run on this computer and offer the options 8gent's start line uses. It is
  installed into 8gent's own folder and the engine it replaces is kept.

Before anything is downloaded, a loader's `[[requires]]` is applied: a version that needs a newer OS
than this computer has is **held**, with the reason (uzu 0.6.17 waits for macOS 26.4).

If the loader is serving, the model is unloaded for the update and loaded again after it. A
prebuilt engine that then fails to load the model is replaced by the one it replaced, and that is
loaded again. 8gent looks for updates a minute after starting and once a day, and with
`auto_update` (on by default, Settings > Update loaders automatically) installs them when nothing
has been asked for half an hour. `8gent updates`, `8gent update <loader>` and the Loaders screen do
the same by hand.

Checked for real: llama.cpp upgraded from 8680 to 0.9.0 through 8gent, with a model loaded
(unloaded, upgraded, loaded again, answering); the prebuilt uzu 0.6.17 engine downloaded, verified
and installed into a test folder. Not yet exercised for real: going back after a prebuilt engine
fails to load.

**Planned for loaders:** building uzu from source when no ready-made engine fits, and TensorFold's
0.6 line, inside 8gent's own folder (both still come from the Home apps' folders); load and unload
commands and the real context window for attached servers (8gent cannot set Ollama's or LM
Studio's window yet, and reports its own default).

## Model library

One entry per model, with a build for each format it exists in. Qwythos-9B-v2 is on the development
Mac three times (GGUF, MLX, a uzu folder); the library shows it once, runnable on three loaders.
Folder names are reduced to one id by dropping format words: `Qwythos-9B-v2-MLX-4bit`,
`Qwythos-9B-v2-GGUF` and `Qwythos-9B-v2` all become `qwythos-9b-v2`. **Built.**

Sources, read at start and on `8gent rescan`:

- `models.toml` in the data folder (written by hand)
- LM Studio's model folder (GGUF files and MLX folders)
- uzu Home's and TensorFold Home's model lists
- each attached server's own model list, asked every 30 s

For each build 8gent reads what it needs for the context window and the memory estimate: the longest
context the model was built for, and the KV-cache bytes one token takes (from the GGUF header, the
Hugging Face `config.json`, or uzu's lalamo config).

**Planned:** downloading models from Hugging Face; converting for uzu from the app; an alias file
for models whose folder names do not match; hiding embedding models from the chat list.

## The endpoint (`/v1`)

**Built**, for `/v1/chat/completions`, `/v1/completions` and `/v1/models`; other `/v1` paths
(including `/v1/messages`) are passed to the engine as they are.

- **Names.** `8gent` (the served name), or any name 8gent does not know, means the selection.
  `loader/model` (`uzu/oxcoder-9b`) or a library model's id names something specific; with
  `auto_switch` on (the default) the service switches to it and that becomes the selection.
- **Waking.** A chat request loads the selected model if it is not loaded. A read (`GET`) never does.
- **One engine at a time.** Requests share a gate; starting, stopping or switching takes it alone,
  so a switch waits for answers in progress and no request loses its engine half way.
- **`/v1/models`** lists the served name first, then every loader and model pair that can run here,
  each with `context_length`, `max_context_length` and `capabilities`. Agents read the window from
  here instead of assuming one.
- **Thinking.** When a request does not choose, the selected mode is applied the way this engine
  takes it. A request's own choice is kept: `enable_thinking: true|false` at the top level works
  with every loader (8gent puts it where that engine reads it). `reasoning_effort` names are mapped
  to ones the engine knows.
- **Roles.** For loaders that need it, a history that resumes with the assistant gets a short user
  turn first.
- **Prompts that do not fit.** Whatever the engine says (a 400, llama.cpp's own error type, uzu's
  error inside a 200 answer or inside a stream), the client gets the OpenAI-standard
  `context_length_exceeded` error with the token count, which Hermes and kr8 answer by compressing
  and retrying. They are counted, with the largest prompt seen.
- **Streams.** The first lines are held for up to 2 s to see whether the engine is about to report
  an error; after that the client has its headers, so a slow prompt read does not look dead. Lines
  pass through without being parsed, except the one that carries the token counts.
- **Speed.** A request that goes through 8gent takes the same time as one sent straight to the
  engine (median of 15 on llama.cpp: 66.2 ms against 66.8 ms).

**Planned:** the Anthropic Messages API for loaders that do not serve it themselves, by translation;
sending headers before the engine has answered at all; shared sampling defaults.

## Control API (`/8gent/v1`)

**Built.** From this computer no key is needed; from another computer it always needs the API key.

| Call | Does |
|---|---|
| `GET /status` | version, selection, engine state (stopped, loading, ready, error) with the reason for an error, memory, totals, recent requests, prompts that did not fit, speed per loader and model |
| `GET /loaders` | every loader: installed, version, reachable, capabilities, requirements and whether this computer meets them, the models it can run |
| `GET /models` | every model: the loaders that can run it (and why not, in words), size, default and longest context, estimated memory |
| `POST /select` | `{loader?, model?, context?, thinking?, load?}`: change the selection |
| `POST /engine/start`, `/engine/stop` | load or unload the selected model |
| `GET /requests?limit=` | the latest requests, newest first |
| `GET /monitor` | one sample a second for the last three minutes: tokens written, requests in progress, engine memory |
| `GET /chats`, `GET`/`PUT`/`DELETE /chats/{id}` | the conversations held in 8gent's own Chat screen (files in `chats/` of the data folder) |
| `GET /logs?loader=&lines=` | the engine's own output |
| `POST /settings` | `{memory_mode?, idle_minutes?, auto_switch?}`, saved at once |
| `POST /rescan` | look again for loaders and models; says which places have not answered yet |
| `POST /shutdown` | stop the service and its engine |
| `POST /small` | `{model, loader?, context?}` or `{off: true}`: the small always-on model |
| `POST /login-item` | `{enabled}`: start at login or not (macOS: a launchd agent of the user) |
| `GET /updates`, `POST /updates/check` | newer engines per loader: current, available, held (with the reason), or updated by its own app |
| `POST /loaders/{id}/update` | install a loader's newest engine; `/updates` shows it running and how it ended |

`/models` gives, for every model and loader pair, what a client needs to price any context window
itself (`base_bytes + kv_bytes_per_token x window x sessions`) and the speed measured so far.
Speeds are read back from the request log when the service starts, so they survive a restart.

### Selecting

- Naming only a model keeps the loader if it can run the model, else takes the first that can.
- The context window belongs to the model: another loader for the same model keeps it, and each
  model's last window is remembered and used again.
- If a model was loaded, the new one is loaded at once. **If it does not come up, the selection goes
  back and what was running is started again**, and the answer carries the engine's own reason.
- Requests that cannot work (unknown model, a loader that cannot run the format, a window larger
  than the model allows) are refused in words before anything is touched.

### Memory

- **Modes:** `idle` (load on the first request, unload after `idle_minutes` without one; the
  default), `on_demand` (load on the first request and keep) and `always` (load when the service starts).
- **Estimate:** weights + about 1.1 GB of runtime + KV bytes per token x context x conversations.
- **Leftovers:** the engine's process is recorded; a service that was killed ends the engine it left
  behind when it next starts. A service that is stopped normally stops its engine first.

- **What else holds memory.** The status lists the other holders of model memory 8gent knows of:
  models loaded in Ollama (with their size) and the earlier Home apps' engines, and what is free
  right now.
- **A load that clearly cannot fit is refused** before what is serving is unloaded: when the
  estimate is more than this computer's GPU can hold at all, or more than is free while one of
  those holders has a model loaded. The answer names the holder ("Qwythos-9B-v2 needs about
  14.7 GB and 11.5 GB is free, because Ollama (llama3.2:3b, 6.0 GB) is holding memory") and can be
  overridden (`force`, "Load anyway"). An estimate that is merely tight is left to the person, and
  a load started by a request is never refused.
- **Measured.** The most an engine was seen to hold is kept per loader and model
  (`state/memory.json`) and shown beside the estimate.

### The small always-on model

A second model, with its own engine, that stays loaded next to the selected one and answers under
the name `8gent-small`: quick acknowledgements and titles while the large model works or is not
loaded (kr8's plan has exactly this: Qwen3.5-0.8B under 1 GB). `8gent small <model>` or Settings
chooses it; it gets an 8,192-token window unless one is named, is loaded at once and at every
start, and is never unloaded for being idle. On the development Mac, Qwen3.5-0.8B on llama.cpp
(0.3 GB) answered in 30 to 60 ms beside OxCoder-9B on TensorFold.

**Planned:** counting LM Studio's loaded models; correcting the estimate from the measured peak;
queuing a load until memory is free.

### Starting

The service answers as soon as it has its port: it reads no model folder and runs no other program
before that. Looking for loaders and models happens in the background, and each place models are
read from is read on its own. This matters on macOS, which stops a program that opens a protected
folder (Documents, Desktop, Downloads) until the person has answered its permission question: a
place that is waiting stays listed under `scanning` in the status, and everything else is there
meanwhile. (The first version waited for the whole scan and sat silent behind that question.)

The status also names other apps that have a model loaded (`also_loaded`: uzu Home, TensorFold
Home), because two loaded models share one computer's memory.

## Command line

**Built.** `8gent serve` runs the service; `status`, `loaders`, `models`, `use <model> [--loader]
[--context 64K] [--thinking on|off]`, `start`, `stop`, `logs` and `rescan` ask a running one.

## First run, and taking over from the Home apps

**Built.** If uzu Home (else TensorFold Home) is installed, 8gent takes over its setup on the first
run: what was selected, with its context window and number of conversations; whether the endpoint is
shared on the network; when the model is kept loaded; and the app's port, which 8gent then answers
on as well (`also_listen`), so whatever pointed at a Home app keeps working. A model the Home app
kept loaded is loaded.

`8gent service install` makes the service start at login (and starts it now); the app's Settings
has the same switch. On macOS this is a launchd agent of the user, marked interactive so that
launchd does not throttle its disk reads (an engine started by a background job took minutes to load
a model). It restarts the service after a crash, not after "Stop the service".

Moving over by hand: switch the Home apps' endpoints and daily updates off (`uzuctl login off`,
`uzuctl gateway stop`, `uzuctl engine stop`, `uzuctl autoupdate off`, and the same with `tfctl`),
then `8gent service install`.

**Planned:** importing their request history and speed logs.

## Testing

`cargo test` runs 23 unit tests and 18 conformance tests; `npm test` in `app/` runs the app's logic
tests. GitHub runs the service tests on macOS, Linux and Windows, and the app's checks, on every push. The conformance tests start the real
service on a spare port, with its own data folder, against `mock-engine`: a stand-in engine whose
quirks are switched on by flags (strict roles, errors inside 200 answers, compact stream lines, slow
prompt reads, a model that will not load). They cover the model list, waking, names, streams and
usage, role repair, the standard overflow error from each style of engine, switching by request and
by `select`, going back after a failed switch, the remembered window, idle unload, an engine that
dies, and the lists and logs.

A new loader is accepted when the same behaviours hold against the real engine. Done so far by hand
for llama.cpp, TensorFold, uzu and Ollama; **planned:** a `8gent check <loader>` command that runs
them.

## The app

**Partly built**, in `app/`: Tauri 2 with React and Tailwind, the stack kr8's desktop uses. It reads
only the control API (inside the window through one `api` command, so the service needs no CORS
rules; in a browser, for development, through Vite's proxy). The direction chosen from three
mock-ups: the model-first screen, with a Home tab that shows what is serving now.

- **Look:** kr8's values, not an imitation of them: the neutral GitHub-derived palette with
  near-black or near-white primary buttons, Inter and JetBrains Mono, 10 px corners and hairline
  borders, lucide icons, the violet sidebar gradient (`#ececf0` to `#e3dcf9` light, `#0a0a0e` to
  `#110e30` dark), kr8's motion timings, and the rainbow "8" as the mark. Light and dark follow the
  system.
- **The window starts the service** if it is not running (the service program is inside the app)
  and leaves it running when the window closes, so the endpoint stays up.

| Screen | State |
|---|---|
| Home | built: what is serving (or selected), speed, memory against this computer's limit, requests answered, load and unload, the "prompts did not fit, use a larger window" offer, the latest requests, a warning when a Home app has a model loaded too |
| Models | built: the list (models 8gent loads itself first, ones only another app serves after), and for the picked model a card per loader with measured speed, memory at the chosen window, size on disk, what it can do, what it needs (for example macOS 26.4), and Switch; a failed switch shows the engine's reason and that 8gent went back |
| Loaders | built: each loader's state, version, formats, capabilities and requirements; "Look again" |
| Requests | built: the latest 200 with tokens, time to first token, speed and result |
| Logs | built: each engine's own output, following the newest line |
| Settings | built: memory mode and idle time, whether requests may switch models, theme, the endpoint address, stopping the service |
| Chat | built, talking straight to the model: streamed answers with Markdown, tables and code blocks with Copy; the model's reasoning folded above its answer; tokens, speed and time to the first token under each answer; stop; attach a text file; thinking on or off; conversations saved and listed. Tools and previews: with the harness layer |
| Monitor | built: tokens written per second and the memory the engine holds over the last two minutes, and what is happening now |
| Race the loaders, profiles, loader install and update | planned |

Checked by hand in a browser and as the built Mac app, against real engines: every screen in both
themes, loading, switching from llama.cpp to TensorFold, a switch to a model whose file is missing
(the app showed llama.cpp's own reason and that 8gent went back to what was running), the settings,
and a chat with and without thinking. Its logic has tests (`npm test`); its screens do not yet. The
app has not been built or run on Windows or Linux.

## Releases

A tag `vX.Y.Z` runs the Build workflow: on GitHub's macOS (Apple Silicon), Windows and Linux
machines it builds the service program and the app, and attaches them to a draft release
(`8gent-X.Y.Z-macos-arm64.dmg` and `.app.zip`, `-windows-x64-setup.exe`, `-linux-x64.AppImage` and
`.deb`, and `8gent-service-...` for the program alone). A person writes the notes and publishes it.
Nothing is signed with a platform certificate yet.

The app asks GitHub for the newest release (Settings > Updates). On macOS it installs it: the
`.app.zip` must match the SHA-256 GitHub publishes for it, be 8gent, and be the version the release
names; then the app quits and a helper keeps a copy of the version that was there, puts the new one
in, restarts the service (the login item runs the program inside the app) and opens the app again.
On Windows and Linux it names the release page.

The downloads also have a page on kr8's site, https://downloads.kr8.app/8gent/.

## kr8

kr8 already reaches local models through one provider, `kr8-local`, which speaks to any
OpenAI-compatible address (default: Ollama's port; `KR8_LOCAL_BASE_URL` overrides it). 8gent fits
behind it with no new transport. What changes in kr8, each in a new kr8 file as its rules ask:

1. Detect 8gent (port 8436) in the capability catalogue and prefer it when present; later make it
   the default address and ship the 8gent binary with kr8.
2. Read `context_length` from `/v1/models` and pass it to the agent, replacing the fixed 32,768 and
   the Ollama-only derived tags. (8gent is not Ollama, so kr8's Ollama cap leaves it alone.)
3. Read `capabilities` from `/v1/models` instead of asking Ollama's `/api/show`.
4. A small settings card: loader, model, state, "Open 8gent".

kr8's plan for a small always-on model next to the main one is the "second slot" above.

## Agent harnesses (planned)

The same pattern one level up. A harness (Hermes, kr8's agent, opencode, Claude Code) is described
by a file, like a loader, and 8gent works with it at three levels:

1. **Configure.** 8gent points the harness at its endpoint (writing the harness's own config, with a
   backup) and shows which harnesses are connected.
2. **Run.** 8gent starts and stops the harness and shows its sessions and logs.
3. **Serve.** 8gent offers a whole agent as an endpoint, so kr8 or another app calls "the agent"
   the way it calls a model.

The Chat screen is the first client of level 2: a conversation in it runs through the harness you
choose, with its tool calls and previews shown. Before the harness layer exists, Chat talks to the
loaded model directly.

kr8 already drives its harnesses through the Agent Client Protocol (its `buzz-acp` bridge, with
`hermes-acp` and Claude Code among the presets), so that protocol is the first candidate for levels
2 and 3. Which protocol each harness really offers is to be checked when this work starts; nothing
of this section is built.

## Next steps

In the order agreed:

1. ~~Chat (direct to the model) and Monitor screens.~~ Done.
2. ~~Daily setup on a Mac: start at login, the Home apps' old ports, their settings taken over.~~
   Done (`8gent service install`, `also_listen`). Their request history is not imported.
3. ~~Engine install and update.~~ Done for Homebrew formulas and ready-made engines; building
   from source and TensorFold's 0.6 line are still to move in.
4. ~~One memory budget, peak-memory measurement, and a second slot for a small always-on model.~~
   Done in a first form (see Memory).
5. ~~Releases with in-app updates (GitHub and a page on the kr8 site); the Home apps' repositories
   then point here.~~ Done: a version tag builds every system's files into a draft release; the
   app checks for and (on macOS) installs its own updates.
6. Windows and Linux: ~~builds~~ done by GitHub; still to do: a person trying them, a job object on
   Windows so an engine never outlives the service, starting at login there, and updates that
   install themselves.
7. kr8: detection and the real context window, on a branch of kr8 with a draft pull request.
8. The harness layer, and with it Chat's agent abilities.

## Open

- **apollo.** The project to match for responsiveness could not be found on this Mac or on GitHub
  (an earlier project's notes say the same). Its location is needed before borrowing from it.
- **Strata on Mac.** Strata 0.2.13 ships a Mac app and a `strata launch` command, but its packs need
  far more memory than the 24 GB development Mac has, so the loader is untested here.
