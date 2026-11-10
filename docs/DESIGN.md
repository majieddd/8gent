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
**Planned:** the app.

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

**Planned for loaders:** installing and updating engines from 8gent (today it uses engines that are
already installed, including the ones the Home apps installed); the uzu build-from-source and
prebuilt-engine update path from uzu Home; load and unload commands and the real context window for
attached servers (8gent cannot set Ollama's or LM Studio's window yet, and reports its own default).

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
  takes it. A request's own choice is kept. `reasoning_effort` names are mapped to ones the engine knows.
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
| `GET /logs?loader=&lines=` | the engine's own output |
| `POST /rescan` | look again for loaders and models |

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

**Planned:** one memory budget across everything (refuse or queue a load that would push the
computer into swap, counting what Ollama, LM Studio and other apps hold); measuring the loaded
engine's peak to correct the estimate, as uzu Home 0.1.2 does; a second slot for a small always-on
model next to the main one.

## Command line

**Built.** `8gent serve` runs the service; `status`, `loaders`, `models`, `use <model> [--loader]
[--context 64K] [--thinking on|off]`, `start`, `stop`, `logs` and `rescan` ask a running one.

## First run

**Built.** If uzu Home (else TensorFold Home) is installed, 8gent selects what it had selected, with
its context window and number of conversations. Nothing is loaded until a request arrives.

**Planned:** importing their request history and speed logs; answering on their old ports (8585 and
8484) so nothing that points at them has to change on the day of the move.

## Testing

`cargo test` runs 16 unit tests and 15 conformance tests. The conformance tests start the real
service on a spare port, with its own data folder, against `mock-engine`: a stand-in engine whose
quirks are switched on by flags (strict roles, errors inside 200 answers, compact stream lines, slow
prompt reads, a model that will not load). They cover the model list, waking, names, streams and
usage, role repair, the standard overflow error from each style of engine, switching by request and
by `select`, going back after a failed switch, the remembered window, idle unload, an engine that
dies, and the lists and logs.

A new loader is accepted when the same behaviours hold against the real engine. Done so far by hand
for llama.cpp, TensorFold, uzu and Ollama; **planned:** a `8gent check <loader>` command that runs
them.

## The app (planned)

Tauri 2 with React, Tailwind and shadcn, the stack kr8's desktop uses, reading only the control API.

- **Look:** kr8's tokens, not an imitation of them: the neutral GitHub-derived palette with
  near-black or near-white primary buttons, Inter and JetBrains Mono, 10 px corners and hairline
  borders, lucide icons, the violet sidebar gradient (`#ececf0` to `#e3dcf9` light, `#0a0a0e` to
  `#110e30` dark), kr8's motion timings, and the rainbow "8" as the mark.
- **Choosing is the main screen.** Pick a model; its loaders appear as cards with measured speed,
  estimated memory against this computer's budget, and what each can do (tools, JSON schema,
  pictures, several conversations). One press switches, with the going-back behaviour above shown
  as it happens.
- **Everything the Home apps had:** Chat, Monitor, Requests, Logs, the context window editor,
  server controls, updates, sharing on the network with a key.
- **New:** a loader page (installed, version, update, requirements such as "needs macOS 26.4"),
  "race the loaders" (the same prompts through each loader that can run a model, to fill the speed
  table), profiles (a named loader + model + window + thinking mode, selectable as a model name).

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

## Agent harnesses (later)

The same pattern one level up: a harness (Hermes, others) described by a file, started and pointed
at 8gent's endpoint by the service, chosen in the app next to the loader and model. Nothing of this
is built; the loader work comes first.

## Next steps

1. The app: design direction as mock-ups, then the choosing screen, status and logs.
2. Engine install and update, starting with the uzu and TensorFold paths the Home apps have.
3. Peak-memory measurement and the memory budget.
4. Windows and Linux builds, with a job object on Windows so an engine never outlives the service.
5. The Home apps' old ports and history import; then their repositories point here.
6. kr8 detection and context length.

## Open

- **apollo.** The project to match for responsiveness could not be found on this Mac or on GitHub
  (an earlier project's notes say the same). Its location is needed before borrowing from it.
- **Strata on Mac.** Strata 0.2.13 ships a Mac app and a `strata launch` command, but its packs need
  far more memory than the 24 GB development Mac has, so the loader is untested here.
