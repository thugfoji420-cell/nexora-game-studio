"""Embed the SxS manifest into every cargo **test harness** exe and
(optionally) run the tests.

Why: tauri_build embeds the app manifest only into the application binaries
(`cargo:rustc-link-arg-bins`). Unit-test and integration-test harness
executables are linked without any manifest, so the Windows loader resolves
comctl32 as the System32 v5 copy, which lacks `TaskDialogIndirect`
(imported statically by tao). The process is killed by the loader with
STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139) before main runs. Embedding a
manifest that activates Common-Controls v6 fixes the launch.

Mechanism: `mt.exe` (Windows SDK Manifest Tool) injects
`scripts/test-harness.manifest` as resource `#1` of each test harness exe.
Targets are discovered from `cargo test --no-run --message-format=json`
("executable" of every message whose target kind is "test"), so the script
never has to guess file names.

Usage:
    python scripts/fix_test_manifest.py                # compile + embed only
    python scripts/fix_test_manifest.py --run -- --lib # compile + embed + run
                                                       #   `cargo test --lib`
    python scripts/fix_test_manifest.py -- --test code_agent -- --run whatever
"""
from __future__ import annotations

import glob
import json
import os
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
SRC_TAURI = REPO / "src-tauri"
MANIFEST = REPO / "scripts" / "test-harness.manifest"


def find_mt_exe() -> Path:
    kits = Path(r"C:\Program Files (x86)\Windows Kits\10\bin")
    candidates = sorted(glob.glob(str(kits / "*" / "x64" / "mt.exe")), reverse=True)
    if not candidates:
        sys.exit("mt.exe not found under Windows Kits — install Windows SDK.")
    return Path(candidates[0])


def compile_test_targets(extra_args: list[str], harness_args: list[str]) -> list[Path]:
    """Run `cargo test --no-run --message-format=json` and return every
    executable path belonging to a test target (unit + integration).

    When everything is already built (fresh), cargo emits no
    compiler-artifact messages, so we fall back to locating the harness exes
    on disk from `cargo metadata` target names."""
    cmd = ["cargo", "test", "--no-run", "--message-format=json"] + extra_args + ["--"] + harness_args
    proc = subprocess.run(cmd, cwd=SRC_TAURI, capture_output=True, text=True, encoding="utf-8", errors="replace")
    if proc.returncode != 0:
        sys.stdout.write("\n".join(proc.stdout.splitlines()[-40:]) + "\n")
        sys.stderr.write(proc.stderr)
        sys.exit(f"cargo compile failed with exit {proc.returncode}")
    executables: list[Path] = []
    for line in proc.stdout.splitlines():
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        if msg.get("reason") != "compiler-artifact":
            continue
        target = msg.get("target", {})
        profile = msg.get("profile", {})
        kinds = target.get("kind", [])
        is_test = "test" in kinds or ("lib" in kinds and profile.get("test", False))
        if not is_test:
            continue
        exe = msg.get("executable")
        if exe:
            executables.append(Path(exe))
    if executables:
        return executables
    print("  (fresh build — no artifact messages; discovering exes from cargo metadata)")
    return discover_from_metadata(extra_args)


def discover_from_metadata(extra_args: list[str]) -> list[Path]:
    """Locate test-harness exes under target/debug|release/deps for our
    package: unit-test harness = `<lib_crate_name>-<hash>.exe`, integration
    tests = `<target_name>-<hash>.exe`. Extra args like --lib scope patterns
    so we never touch unrelated build artifacts."""
    meta = subprocess.run(
        ["cargo", "metadata", "--format-version", "1"],
        cwd=SRC_TAURI, capture_output=True, text=True, encoding="utf-8", errors="replace", check=True,
    )
    data = json.loads(meta.stdout)
    ours = [p for p in data.get("packages", []) if p["name"] == "nexora-game-studio"]
    patterns: list[str] = []
    for pkg in ours:
        for target in pkg.get("targets", []):
            kinds = target.get("kind", [])
            name = target.get("name", "")
            # "lib" here is substring-level: the lib target's kinds are
            # ["staticlib", "cdylib", "rlib"], none of which equals "lib".
            if any("lib" in k for k in kinds):
                patterns.append("nexora_game_studio_lib-*.exe")
            if "test" in kinds and extra_args != ["--lib"]:
                patterns.append(f"{name}-*.exe")
    depts = SRC_TAURI / "target" / "debug" / "deps"
    found: list[Path] = []
    for pattern in patterns:
        found.extend(sorted(depts.glob(pattern)))
    return [p for p in found if p.is_file()]


def embed_manifest(mt: Path, exe: Path) -> str:
    result = subprocess.run(
        [str(mt), "-nologo", "-manifest", str(MANIFEST),
         f"-outputresource:{exe};1"],
        capture_output=True, text=True,
    )
    return (result.stdout + result.stderr).strip()


def main(argv: list[str]) -> int:
    run = "--run" in argv
    rest = [a for a in argv[1:] if a != "--run" and a != "--"]
    # Args split like cargo does: sab kuch pehla `--` tak compile step me jata
    # hai (filters/subcommands), uske baad ka har cheez test-runner ke paas.
    if "--" in rest:
        cut = rest.index("--")
        extra, harness_args = rest[:cut], rest[cut + 1:]
    else:
        extra, harness_args = rest, []
    mt = find_mt_exe()
    print(f"mt.exe: {mt}")

    print("compiling test targets (cargo test --no-run) ...")
    exes = compile_test_targets(extra, harness_args)
    if not exes:
        print("no test-harness executables found — nothing to patch.")
        return 0

    patched = 0
    for exe in exes:
        msg = embed_manifest(mt, exe)
        status = "embedded" if not msg else f"mt said: {msg}"
        print(f"  {exe.name}: {status}")
        patched += 1
    print(f"patched {patched} test-harness exe(s).")

    if run:
        print("running tests: cargo test", " ".join(extra + harness_args) or "(default all)")
        cmd = ["cargo", "test"] + extra + ["--"] + harness_args
        return subprocess.run(cmd, cwd=SRC_TAURI).returncode
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
