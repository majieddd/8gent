# 8gent

**One local endpoint in front of any model loader.** Choose a loader (uzu, TensorFold, llama.cpp,
Ollama, LM Studio, Strata) and a model; Hermes, kr8 and every other app keep the same address
whatever you choose: `http://127.0.0.1:8436/v1`.

8gent continues uzu Home, TensorFold Home and StrataHome as one modular project, and is part of the
kr8 family. This repository holds the service, its command line and the first version of the app.
What is built and what is planned is in [docs/DESIGN.md](docs/DESIGN.md).

**Status: early.** The service runs and is tested with real models on llama.cpp, TensorFold, uzu
and Ollama on an Apple Silicon Mac. It uses engines that are already installed; it does not install
or update them yet. The app builds and runs on that Mac; there is no release yet, and nothing has
been built on Windows or Linux.

## What it does today

- **One model, several loaders.** A model that is on disk as GGUF, as MLX and as a uzu folder is one
  entry, runnable on three loaders. 8gent reads LM Studio's model folder, the Home apps' model
  lists, and what Ollama and LM Studio serve.
- **Switch with one command or one request.** `8gent use qwythos-9b-v2 --loader uzu`, or send a
  request with the model `uzu/qwythos-9b-v2`. If the new engine does not come up, 8gent goes back
  to what was running and tells you the engine's own reason.
- **Loads on demand, unloads when idle**, and never leaves an engine behind when it stops.
- **Every loader behaves the same to clients:** the real context window in `/v1/models`, the
  standard `context_length_exceeded` error when a prompt does not fit (so agents compress and
  retry), thinking mode applied the way each engine takes it, histories repaired for engines that
  refuse them.
- **Measures:** requests, tokens, time to the first token and tokens per second, kept per loader
  and model, so you can see which loader runs a model fastest on your computer.
- **Adds nothing to the time:** a request through 8gent takes as long as one sent straight to the
  engine.

## Try it

Needs Rust (built and tested with 1.98) and at least one loader installed.

```bash
cargo build --release
```

```bash
target/release/8gent serve
```

In another terminal:

```bash
target/release/8gent loaders
```

```bash
target/release/8gent models
```

```bash
target/release/8gent use qwen3.5-0.8b --context 32K
```

```bash
curl http://127.0.0.1:8436/v1/chat/completions -H 'Content-Type: application/json' -d '{"model":"8gent","messages":[{"role":"user","content":"Hello"}]}'
```

`8gent status` shows what is selected and loaded, the memory it holds, and the speed measured so
far. `8gent help` lists every command.

On its first run 8gent selects what uzu Home (or TensorFold Home) had selected, if one is installed.
Nothing loads until a request arrives. **Do not load the same large model in a Home app and in 8gent
at once:** each keeps its own copy in memory.

## The app

In [app/](app/): Tauri with React and Tailwind, in kr8's look. Home shows what is serving; Models
shows, for the model you pick, every loader that can run it with its measured speed and the memory
it needs at the context window you choose, and switches with one press. Chat talks to the loaded
model; Monitor, Loaders, Requests, Logs and Settings are there too. It needs Node (built with 22) and Rust.

```bash
cargo build --release
```

```bash
cd app && npm install && npx tauri build
```

The app is then at `app/src-tauri/target/release/bundle/macos/8gent.app`. Opening it starts the
service if it is not running; closing the window leaves the service running, so the endpoint stays
up (Settings has "Stop the service"). To work on the screens in a browser: `8gent serve`, then
`npm run dev` in `app/` and open http://localhost:1420.

On macOS, the first time 8gent reads models kept in Documents, Desktop or Downloads, macOS asks
whether it may. Until you answer, only those models are missing; the rest of 8gent works.

## Loaders

| Loader | How 8gent uses it | Runs |
|---|---|---|
| llama.cpp | starts `llama-server` | GGUF |
| TensorFold | starts `tensorfold serve` (and the 0.6.6 Python line as variant `tf06`) | MLX |
| uzu | starts uzu Home's patched `uzu server` | converted folders, Mirai's builds |
| Ollama | uses Ollama's running server | what Ollama has pulled |
| LM Studio | uses LM Studio's local server | what LM Studio has loaded |
| Strata | uses Strata's running server | Strata's packs |

A loader is one TOML file ([loaders/](loaders/)); see [the design](docs/DESIGN.md#loaders) for the
format. Put your own in the `loaders` folder of 8gent's data folder.

## Where things are

| | |
|---|---|
| Data folder | macOS `~/Library/Application Support/8gent`, Linux `~/.local/share/8gent`, Windows `%APPDATA%\8gent`; or wherever `EIGHTGENT_HOME` points |
| `config.toml` | port, selection, memory mode, per-loader overrides |
| `models.toml` | models you add by hand |
| `logs/engine-<loader>.log` | each engine's own output |
| `state/requests.jsonl` | one line per request answered |

## Tests

```bash
cargo test
```

18 unit tests and 15 conformance tests. The conformance tests run the real service against
`mock-engine`, a stand-in engine that reproduces each loader's quirks without a model or a GPU.
`npm test` in `app/` runs the app's logic tests.

## Source

| | |
|---|---|
| `src/manifest.rs` | loader definitions |
| `src/library.rs` | the model library and what is read from model files |
| `src/engine.rs` | planning, starting, watching and stopping an engine |
| `src/server.rs`, `src/proxy.rs` | the control API and the endpoint |
| `src/normalize.rs` | making every loader's API behave the same |
| `src/stats.rs` | requests and speed |
| `src/main.rs` | the `8gent` command |
| `src/bin/mock_engine.rs`, `tests/conformance.rs` | the stand-in engine and the tests |
| `app/src` | the app's screens (React); `app/src/lib/api.ts` is its one way to the service |
| `app/src-tauri` | the window (Tauri): the `api` command and starting the service |

Not affiliated with Mirai, TensorFold, llama.cpp, Ollama, LM Studio or Strata. MIT licence.
