# Offline Self-Coding Mode — User Guide

Nexora Game Studio can act as its own coding assistant **without any API key
and without any internet connection**. A small, powerful coding model
(Qwen2.5-Coder) runs entirely on your PC. Chat, code questions and work
orders keep working with the network cable pulled out.

---

## What it is

Nexora bundles a fifth AI provider alongside OpenAI / Anthropic / Gemini /
Custom: **🏠 Offline local**. Instead of calling a cloud API it talks to an
inference engine on your own machine:

- **Engine (zero setup, packaged app):** llama.cpp `llama-server`, shipped
  inside the app as a Tauri sidecar. Nothing to install — the app spawns it
  on `127.0.0.1:8080` when you enable the offline mode.
- **Engine (alternative):** if you already use [Ollama](https://ollama.com),
  LM Studio, or vLLM, Nexora finds them automatically (`localhost:11434`,
  `:1234`, `:8000`) and uses them instead — no duplicate installs.
- **Model:** Qwen2.5-Coder-7B-Instruct (Apache-2.0), quantized to Q4_K_M,
  ~4.7 GB, downloaded **once** with a SHA-256 checksum guarantee.

While the offline model answers, **no outbound request is made** — inference
is purely `localhost` loopback traffic.

---

## One-click guided setup

Open **AI Settings** (dashboard → ⚙️ AI Settings) and find the green
**Offline local model** card at the top. Click:

> **🚀 Set up offline AI (guided, one click)**

The flow narrates each step live in the card:

| Stage | What happens |
|---|---|
| `probe` | Looks for a running engine among the known loopback endpoints. If one already answers, setup finishes here. |
| `model` | Bundled engine: downloads the GGUF (~4.7 GB) once into your app-data folder, with a live progress bar and SHA-256 verification (`1664fcca…956b6`). Ollama engine: streams `ollama pull qwen2.5-coder:7b` into the same progress bar. |
| `engine` | Starts llama-server (or `ollama serve`) and waits until it answers on the loopback port. First start also loads ~4.7 GB into RAM/VRAM — give it a minute. |
| `ready` | Nexora switches the active provider to **Offline local** automatically and saves it. The next chat message runs on your own GPU/CPU. |

Advanced users who prefer manual control get two extra buttons:
**⬇ Download model only** and **▶ Start engine only**.

If you pick a 🏠 local model from the dashboard's model dropdown while no
engine is running, the chat itself explains the guided setup so you never
have to hunt for the setting.

---

## Hardware requirements

These are *practical planning* figures. Real usage depends on context length,
drivers and how much of the model stays on the GPU.

| Your machine | Recommended model | What to expect |
|---|---|---|
| 4 GB VRAM GPU, or 8 GB RAM laptop | `qwen2.5-coder:1.5b` or `qwen2.5-coder:3b` | Quick answers, weaker on multi-file reasoning. |
| 6–8 GB VRAM GPU, 16 GB RAM | **`qwen2.5-coder:7b`** *(default)* | Good everyday coding — work orders, Unity C# snippets, explanations. |
| 12+ GB VRAM | `qwen2.5-coder:14b` | Noticeably better reasoning; slower first token. |
| CPU only (no GPU) | 7B works, 1.5b feels snappier | Plan on 16 GB system RAM for a comfortable 7B experience. |

Notes:

- The app launches the bundled engine with `-ngl 0` (CPU serving). If you
  have a GPU and prefer full offload, use Ollama/LM Studio which manage
  GPU layers for you, or run `llama-server` with your own `-ngl` value.
- Disk: budget ~5 GB in `%LOCALAPPDATA%\NexoraGameStudio\localai\models`
  for the bundled path (the model never lives in the install folder, so app
  updates can't wipe it).
- First response after a cold start is much slower than later ones — that's
  the model loading into memory, not the CPU choking.

---

## Choosing the model in chat

The dashboard model bar lists **🏠 Offline local** models first:

- `qwen2.5-coder:7b` — default;
- `qwen2.5-coder:14b`, `:3b`, `:1.5b` — size/quality tradeoffs;
- `phi3.5:3.8b` (MIT) — general chat, not code-specialized;
- `deepseek-coder-v2:16b-lite-instruct-q4_K_M` — strong coder, needs ~10 GB.

Per-Dashboard-mode memory still applies: Agent, Codex and Midjourney each
remember their own last-used model, so you can permanently put Codex on the
offline coder while Agent stays on a free cloud model.

**Licensing note:** Qwen2.5-Coder's 3B variant is *research-licensed* and is
deliberately not offered; everything else listed is Apache-2.0 or MIT.

---

## How the fallback chain helps

With **Auto-switch on quota hit** enabled (default), a configured Offline
local provider participates in automatic failover — and it needs **no API
key** to qualify. Cloud quota exhausted or the internet down? The chat
silently lands on your local brain and badges the reply as a fallback.

---

## Privacy: what leaves your PC

- **During inference: nothing.** All engine traffic is `127.0.0.1` loopback.
- **One-time outbound:** the model download (Hugging Face URL pinned in the
  app) or an Ollama registry pull. That's the only network use.
- No telemetry is attached to the local engine, no prompts are logged to
  disk beyond the normal chat-history database you already control.

---

## Troubleshooting

| Symptom | Fix |
|---|---|
| *"Local AI runtime not reachable"* while chatting | The engine died or never started. Open AI Settings → run 🚀 guided setup again; for Ollama, run `ollama serve` in a terminal to see the real error. |
| Model card says *checksum mismatch* | The download was corrupted. Delete `%LOCALAPPDATA%\NexoraGameStudio\localai\models\*.part` and re-run the guided setup. |
| `:8080` already in use (another llama-server/LM Studio) | Free the port, or point the Custom provider at whatever engine exists; the status probe will find the live one automatically. |
| Setup stuck at *engine* with Ollama | Ollama is installed but not starting — `ollama serve` in a terminal shows why (usually a port clash or stale process). |
| Answers are slow on CPU | Pick a smaller model (`:3b`/`:1.5b`) in the dropdown, or install Ollama to get automatic GPU offload. |
| Everything offline but chat says "no model configured" | Open AI Settings; if the offline card shows *RUNNING*, click **Set Active** (or the guided flow does it for you). |

Under the hood the status command probes, in order:
`localhost:11434` (Ollama), `:8080` (bundled llama-server), `:1234` (LM
Studio), `:8000` (vLLM), `:1337`. Any one of them answering OpenAI-compatible
`/v1/chat/completions` works.

---

## Power-user cheat sheet

```text
Model store        %LOCALAPPDATA%\NexoraGameStudio\localai\models
Engine (packaged)  <install>\llama-server.exe -m <model> --host 127.0.0.1 --port 8080 -ngl 0 --no-webui
Engine (Ollama)    ollama serve          # then ollama pull qwen2.5-coder:7b happens automatically
Progress events    llama-model-download  (percent, verifying, Ollama status text)
Stage events       local-ai-setup        (probe | model | engine | ready × running | done | error)
```

The setup flow is one Tauri command — `local_ai_setup` — so scripts and
future onboarding screens can trigger the exact same path.
