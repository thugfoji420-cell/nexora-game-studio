"""A/B test two Ollama models on the same coding prompts.

Same endpoint path the app's local provider uses (OpenAI-compatible
/chat/completions on the saved Ollama port), same sampling params, timed runs.
Output: per-prompt wall time, tokens, tok/s and the full replies so quality
can be judged side by side.

Usage: python scripts/ab_test_models.py [modelA modelB]
Defaults to qwen2.5-coder:3b vs qwen2.5-coder:7b on localhost:11434.
"""
import json
import sys
import time
import urllib.request
from pathlib import Path

BASE = "http://localhost:11434"
MODELS = (sys.argv[1], sys.argv[2]) if len(sys.argv) > 2 else ("qwen2.5-coder:3b", "qwen2.5-coder:7b")

# Difficulty-ramped coding prompts: SQL reasoning, bug handling, edge-case logic.
PROMPTS = {
    "P1_sql_top_products": (
        "Write ONE SQLite query: find the 3 most-ordered products from table "
        "orders(id, product_name, quantity), sorted by total quantity. "
        "Return SQL only, no explanation."
    ),
    "P2_bugfix_average": (
        "Fix this Python function so it never crashes: "
        "def average(nums): return sum(nums) / len(nums)  "
        "Return the fixed code plus a one-line docstring only."
    ),
    "P3_rust_chunk_ranges": (
        "Write a Rust function: fn chunk_ranges(total: usize, chunks: usize) "
        "-> Vec<(usize, usize)> returning [start, end) ranges that evenly "
        "cover 0..total. Handle chunks == 0 safely. Code only."
    ),
}


def ask(model: str, prompt: str, max_tokens: int = 250) -> dict:
    req = urllib.request.Request(
        f"{BASE}/v1/chat/completions",
        data=json.dumps({
            "model": model,
            "messages": [
                {"role": "system", "content": "You are a precise coding assistant. Follow the user's format exactly."},
                {"role": "user", "content": prompt},
            ],
            "max_tokens": max_tokens,
            "temperature": 0.2,  # deterministic-ish for fair A/B
        }).encode(),
        headers={"Content-Type": "application/json"},
    )
    t0 = time.time()
    r = json.load(urllib.request.urlopen(req, timeout=180))
    dt = time.time() - t0
    u = r["usage"]
    return {
        "model": model,
        "wall_s": round(dt, 1),
        "tokens": u["completion_tokens"],
        "tok_s": round(u["completion_tokens"] / dt, 1),
        "reply": r["choices"][0]["message"]["content"].strip(),
    }


def main() -> None:
    results = {}
    for model in MODELS:
        results[model] = {}
        for key, prompt in PROMPTS.items():
            results[model][key] = ask(model, prompt)
            print(f"{model} :: {key} -> {results[model][key]['wall_s']}s, "
                  f"{results[model][key]['tok_s']} tok/s", flush=True)
    out_path = Path(__file__).with_suffix(".results.json")
    with open(out_path, "w", encoding="utf-8") as f:
        json.dump({"prompts": list(PROMPTS), "results": results}, f, indent=1, ensure_ascii=False)
    print(f"saved {out_path}")
    # Compact table
    print("\n== SPEED TABLE ==")
    for key in PROMPTS:
        row = " | ".join(f"{m.split(':')[1]}: {results[m][key]['tok_s']} tok/s, {results[m][key]['wall_s']}s" for m in MODELS)
        print(f"{key:22s} {row}")


if __name__ == "__main__":
    try:
        main()
    except BrokenPipeError:
        pass
