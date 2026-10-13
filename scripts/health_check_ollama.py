"""Nexora local-AI health check.

Ek jagah se pura health ka pata:
  1. Ollama API alive (/api/tags) on the saved local provider port
  2. App config (llm_config.json) ka local provider -> expected model abhi pulled?
  3. Ollama processes (ollama.exe + ollama app.exe) aur app process info
  4. GPU memory (nvidia-smi — optional, missing pe warn only)

Exit codes: 0 = healthy, 1 = unhealthy. Idempotent — reuse in CI/scripts.
Usage: python scripts/health_check_ollama.py [--port 11434]
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import urllib.request
from pathlib import Path

DEFAULT_PORT = 11434
APP_DATA = Path(os.environ.get("LOCALAPPDATA", "")) / "com.nexora.gamestudio"
CONFIG_PATH = APP_DATA / "llm_config.json"


def read_config_provider() -> dict | None:
    """App ka local provider config — model + baseUrl live source of truth."""
    try:
        cfg = json.loads(CONFIG_PATH.read_text(encoding="utf-8"))
        provider = cfg.get("providers", {}).get("local", {})
        return {"model": provider.get("model"), "baseUrl": provider.get("baseUrl"),
                "active": cfg.get("activeProvider") == "local"}
    except (OSError, json.JSONDecodeError):
        return None


def api_models(port: int) -> list[str] | None:
    """Ollama API se models; None = API not reachable."""
    try:
        with urllib.request.urlopen(f"http://localhost:{port}/api/tags", timeout=5) as resp:
            data = json.load(resp)
        return [m.get("name", "") for m in data.get("models", [])]
    except (OSError, json.JSONDecodeError):
        return None


def processes_running() -> dict[str, bool]:
    out = subprocess.run(
        ["tasklist", "/FO", "CSV"], capture_output=True, text=True
    ).stdout.lower()
    return {
        "ollama server (ollama.exe)": "ollama.exe" in out,
        "ollama tray app (ollama app.exe)": "ollama app.exe" in out,
        "nexora app (nexora-game-studio.exe)": "nexora-game-studio.exe" in out,
    }


def gpu_note() -> str:
    try:
        out = subprocess.run(
            ["nvidia-smi", "--query-gpu=memory.used,memory.total,utilization.gpu",
             "--format=csv,noheader"], capture_output=True, text=True, timeout=10,
        ).stdout.strip()
        return f"GPU: {out}" if out else "GPU: nvidia-smi kein output"
    except (OSError, subprocess.TimeoutExpired):
        return "GPU: info unavailable (nvidia-smi missing?)"


def main(argv: list[str]) -> int:
    port = 11434
    if "--port" in argv:
        port = int(argv[argv.index("--port") + 1])
    problems: list[str] = []

    provider = read_config_provider()
    expect_model = (provider or {}).get("model")
    base_url = (provider or {}).get("baseUrl")
    print("== Nexora local AI health ==")
    print(f"config: local model={expect_model} baseUrl={base_url} active={bool(provider and provider.get('active'))}")
    if not provider:
        problems.append(f"app config load nahi hua: {CONFIG_PATH}")

    models = api_models(port)
    if models is None:
        print(f"API: FAIL - Ollama port {port} par jawab nahi dega")
        problems.append("Ollama API reachable nahi hai — `ollama serve` chalao ya tray app start")
    else:
        print(f"API: OK - models: {models}")
        if expect_model and not any(m == expect_model or m.startswith(expect_model) for m in models):
            problems.append(f"expected model '{expect_model}' pulled nahi hai — `ollama pull {expect_model}`")

    for name, up in processes_running().items():
        print(f"process: {'OK  ' if up else 'DOWN'} {name}")

    print(gpu_note())

    if problems:
        print("\n== UNHEALTHY ==")
        for p in problems:
            print(f"  - {p}")
        return 1
    print("\n== HEALTHY ==")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
