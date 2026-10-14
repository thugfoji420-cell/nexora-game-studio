# Nexora application audit and hardening

Date: 2026-10-12

## Scope and workspace provenance

This is a targeted application audit and improvement pass, not a certification that all bugs are fixed or feature parity with Freebuff/Codex has been achieved.

The isolated worktree started at `80d159b`, while the user's current main project was at `347624f` with additional uncommitted agent changes. Current tracked application files and the four code-agent source/migration files were copied into the isolated worktree before auditing. Existing user work was preserved; the main project folder was not edited. User databases, lock files, `.nexora` data and `.freebuff` state were not imported. Ignored local llama engine binaries were copied for compilation only, not added to version control. Consequently, the overall Git diff includes earlier application changes, not just this audit.

## Acceptance criteria

- Preserve the Tauri/Rust + React/TypeScript application and its independent asset-engine workflows.
- Enforce a shared project-file boundary for the code agent and legacy code-file IPC commands.
- Reject traversal, absolute/UNC/drive-relative paths, Windows alternate streams/device names, hidden control directories, and common credential filenames.
- Support new nested code files without following existing symlinks/junctions.
- Default agent runs to read-only; edits require a user-controlled flag and matching project identity.
- Provide cancellation, bounded tool steps, bounded response/read sizes and time limits, and truthful result status.
- Do not execute generated commands or elevate privileges to imitate a full coding agent.
- Improve dashboard readability, keyboard access and small-window navigation without clipping the composer.
- Verify frontend checks, backend compilation, focused filesystem tests and browser UI; distinguish compiled tests from executed tests.

## Findings and implemented repairs

| Priority | Finding | Repair / evidence |
|---|---|---|
| High | Legacy workspace and Unity paths were checked lexically, allowing links/junctions to escape the intended tree. | Shared `workspace_files` resolver rejects links/reparse points on existing ancestors. Unity script access is restricted to `Assets/*.cs`. Directory listing uses the same boundary and stops at its cap. |
| High | Agent `canonicalize` required the final file to exist, making new-file creation fail. | Validates existing ancestors and permits missing descendants; regression covers nested creation and replacement. |
| High | Agent launched PATH-resolved Python/Node checkers with inherited environment, no timeout, and `/dev/stdin` arguments incompatible with Windows. Python module resolution could load project-local modules; pipes could stall. | Removed host checker execution. JSON is validated in-process. Other languages are marked **unchecked**, not passed. This intentionally reduces claimed validation until an isolated checker exists. |
| High | Files were overwritten before validation, using truncating writes (including hardlinks). | JSON validation runs first; rejected JSON is not written. Writes use exclusive temporary files, sync, and same-directory atomic rename. Tested hardlink replacement leaves its original source unchanged. |
| High | Agent could edit without explicit permission, overlap another run or continue using a stale UI project. | Read-only default, explicit `allowWrites`, expected project UUID, one-run semaphore, run-specific cancellation ID, execution-enabled checks. No automatic alternate write path after agent failure. |
| High | WebView options disabled GPU sandboxing and exposed remote debugging on port 9222; script CSP allowed inline/eval/data scripts. | Removed sandbox/debug switches; disabled automatic devtools; restricted scripts to self. Removed inline diagnostics that could throw while handling errors after React removed their DOM nodes. Native WebView behavior still requires a desktop smoke test. |
| High | Four known npm dependency vulnerabilities, including critical Vitest-related findings. | Upgraded Vitest to 4.1.11 and patched transitive source-map-js. `npm audit` reports zero known vulnerabilities for the installed tree; this is not proof of no unknown issues. |
| Medium | Local coding context could be sent to a configured cloud fallback; selected model IDs were overwritten by fallback ranking. | Local/loopback active providers no longer fall back to cloud; active/custom/local model selections are retained. Explicit cloud configurations can still send context to cloud. |
| Medium | Keyless local OpenAI-compatible servers were advertised but rejected by the generic chat function. | Keyless HTTP loopback custom endpoints now work; configured local tokens are retained. Remote custom endpoints require HTTPS without URL credentials/query/fragment; HTTP redirects are not followed. |
| Medium | Gemini API key was embedded in a request URL, risking inclusion in URL/error logs. | Use the `x-goog-api-key` request header. |
| Medium | Model responses and source reads were unbounded; UTF-8 truncation could panic. | Streamed response cap: 2 MiB. Bounded code reads reject oversize files (16 KiB for agent context, 512 KiB for manual reads), avoiding unsafe byte slicing. Error bodies are capped for display. |
| Medium | Tool parsing failed when malformed prose objects preceded the actual tool call; permissive recovery must not allow multiple calls. | Extract balanced JSON objects with string/escape handling, bounded candidates and response size; accept exactly one valid typed call. Original malformed-prefix regression retained, plus dedicated executed protocol tests. |
| Medium | Focus files suppressed the initial task; fallback reset the step budget; failures could be cleared by unrelated reads/writes and then declared successful. | Always sends initial task/focus/mode. Eight-step budget shared across providers, 120-second maximum model wait, five-minute run budget. Unresolved file errors prevent success and reach the model. Actual provider/model/fallback are recorded. |
| Medium | Chat append/update/clear IPC could race and silently lose a fast response. | Ordered frontend history queue with failure propagation and two regression tests. Input waits for history restoration; clear is disabled while busy; load/clear failures are shown. Some older persistence callers still suppress their own errors (see roadmap). |
| Medium | Small-window stylesheet completely hid navigation, and the dashboard composer could extend into clipped space. | Compact accessible icon rail, named buttons/current-page state, deliberate scroll fallback for short windows, keyboard focus indicators and skip link. |
| Medium | Startup event-listener failures could reject unhandled in browser preview; users could remain behind startup while engines failed. | Listener failures handled; browser preview avoids backend polling; Continue to studio control allows engine startup in background. |
| Low | Chat forced smooth scrolling on every update, competing with reading older messages. | Follow-bottom only while near bottom, instant movement, Latest message action. |
| Low | Initial bundle eagerly included the 3D viewer and generator screens. | Lazy-load asset/generator/review workspaces with Suspense fallback. Heavy viewer is deferred. |

### Key changed files

- [Workspace file boundary](../src-tauri/src/workspace_files.rs)
- [Agent loop and IPC](../src-tauri/src/code_agent.rs)
- [Tool protocol parser](../src-tauri/src/code_agent_protocol.rs)
- [Legacy code-file commands](../src-tauri/src/unity_scripts.rs)
- [LLM transport and fallback](../src-tauri/src/llm.rs)
- [WebView configuration](../src-tauri/tauri.conf.json)
- [Dashboard](../src/pages/StudioChatPage.tsx)
- [Ordered history service](../src/services/chatHistory.ts)
- [Workspace lazy loading](../src/App.tsx)
- [Dashboard/accessibility styling](../src/styles.css)
- [Filesystem integration test](../src-tauri/tests/workspace_boundary.rs)

## Agent IPC additions

`code_agent_run` still accepts `{ request }`. Existing schemaVersion is 1; newly enforced fields are:

```ts
{
  schemaVersion: 1,
  task: string,
  requestId: string,          // client-generated UUID
  expectedProjectId: string,  // current UI project's UUID
  allowWrites: boolean,      // false by default in Rust
  maxSteps?: number,
  focusFiles?: string[]
}
```

`code_agent_cancel({ requestId })` cancels that active run. Stop does not undo previously saved files. Leaving the dashboard requests cancellation. Failure/cancelled/needs_input are returned result statuses; IPC/setup errors reject. The frontend must inspect status, not treat every resolved Promise as success.

## Verification

| Check | Result |
|---|---|
| `npm run typecheck` | Passed |
| `npm test` | 108 tests passed in 16 files |
| `npm run build` | Passed; a deferred 586 KB Three.js viewer chunk still produces a size warning |
| `npm audit` | Zero known vulnerabilities |
| `cargo check --manifest-path src-tauri/Cargo.toml` | Passed; existing dead-code/unused-field warnings remain |
| `cargo test --manifest-path src-tauri/Cargo.toml --test workspace_boundary` | Eight tests executed and passed on Windows: four filesystem tests including real junction creation/rejection, and four protocol parser regressions |
| `cargo test --manifest-path src-tauri/Cargo.toml --lib` | Compiles, but executable fails before tests start with `0xc0000139 STATUS_ENTRYPOINT_NOT_FOUND`; full Rust tests are **not passed** (superseded by the wrapper rows below) |
| `npm run test:rust` (`scripts/fix_test_manifest.py --run -- --lib`) | Harness exes patched with mt.exe before every run; 255 unit tests passed, 0 failed, exit 0. Wired into `npm test` and `npm run ci`, so bare `cargo test` is never invoked by automation |
| `npm run test:rust:all` | Same wrapper with full `cargo test`: 255 unit + 8 integration (`workspace_boundary`) tests passed, 0 failed, exit 0 |
| Migration 14 scratch SQLite | Schema version and foreign-key rejection verified |
| Browser preview | Startup entry, dashboard, settings navigation, read-only disabled without desktop/project, browser-only errors, preserved input, sidebar labels, 1280×800 layout and 720×520 scroll fallback verified |
| Fresh dashboard browser console | No application errors after preview startup repair |
| Native app + real models/asset runtimes | Not executed; requires desktop test and configured local/cloud providers |

New backend tests include the real agent loop against a local scripted OpenAI-compatible HTTP server: create file, initial task with focus files, invalid JSON, read-only denial, step exhaustion and pre-cancellation. These tests **compile but have not executed**, because they are in the blocked full library harness. They have not been ignored or bypassed. The independent workspace test target executes production filesystem and protocol-parser code without loading the UI library harness. During final review, a changed parser assertion was restored to the original malformed-prefix input, and the parser itself was repaired rather than weakening the assertion.

## Measured efficiency

Same workspace, same updated frontend dependencies, before/after workspace lazy loading:

| Artifact | Before | After |
|---|---:|---:|
| Initial minified JS | 1,078.07 KB | 344.70 KB |
| Initial JS gzip | 288.91 KB | 103.48 KB |

Initial JS is approximately 68% smaller (64% smaller compressed). This is a bundle-size reduction, **not a measured runtime speedup**. Production builds passed, but timings vary with warm caches and are not claimed as performance improvements. Core Web Vitals/Lighthouse/trace metrics were not measured: Chrome DevTools MCP performance tools are unavailable in this session.

## Important remaining risks / incomplete work

1. **Not an OS sandbox.** Canonical checks/reparse rejection do not make the filesystem immune to a hostile local process swapping ancestors between check and use (TOCTOU). Hardlinks can still expose data on reads. Strong isolation needs handle-relative no-follow file APIs or an OS-isolated execution workspace.
2. **Source can contain secrets.** Hidden paths/common credential names are denied, but the system cannot recognize arbitrary credentials embedded in an otherwise allowed source file. Cloud providers are explicitly configured and may receive requested file contents.
3. **Key storage remains plaintext configuration.** Move credentials to the OS credential store and return redacted provider data where possible; current frontend settings APIs intentionally expose configured keys to the local UI.
4. **No safe generated-command runner exists.** The code agent deliberately does not execute commands or generated code. Implement explicit approvals plus real isolation before adding installs, shell commands or test execution. Never run the app elevated.
5. **Permission is per run, not per diff.** Enabled edits can still replace arbitrary allowed project source/config files. There is no diff approval, checkpoint, rollback or conflict check against external edits. Atomic single-file writes are not a multi-file transaction.
6. **Cancellation scope.** The new Stop action covers code-agent runs, not every image/video/pipeline job. A currently executing small synchronous file write can finish before Stop is observed.
7. **Crash recovery and streaming.** The backend persists run rows but startup recovery of interrupted code-agent runs is not implemented. The timeline is returned after completion rather than streamed live. User questions do not yet resume the same run.
8. **Project/session lifecycle.** Workspace navigation remounts pages; code-run cleanup requests cancellation, but broader asset pipeline navigation and overlapping engine-project transitions need dedicated integration coverage.
9. **History scale and error UX.** History load is still unbounded; pagination/virtualization and a persistent unsaved-status indicator are needed. Ordered mutations repair races but do not make disk errors disappear.
10. **Other application surfaces are not certified.** Destructive project/asset deletion, all provider adapters, sidecar/model download provenance, installer packaging, native CSP/3D rendering and real Unity delivery require a separate native/integration pass. Generic startup fallbacks can still mask backend initialization failures.
11. **No native release was produced.** Changes and preview are in the isolated worktree; the installed application and main project are unchanged. No commit, push or deployment was performed.

## Recommended next milestones

1. Diagnose the Windows library-harness loader failure; execute the full existing and new backend suite and a native create-project → code prompt → inspect files → Stop workflow.
2. Introduce proposed-edit diffs, explicit Apply, conflict detection and restorable checkpoints before claiming premium autonomous coding capability.
3. Implement isolated command/test execution with approvals, bounded output/process trees and trustworthy runtime paths; no elevation.
4. Add streamed steps, resumable questions, conversation sessions, searchable paginated history and project-aware memory.
5. Add native end-to-end tests for each image/video/3D/approval/Unity path and a release packaging smoke test.

## Dependency research references

- https://github.com/advisories/GHSA-82fw-gwwq-j7x9
- https://github.com/vitest-dev/vitest/releases/tag/v4.1.11
- https://github.com/advisories/GHSA-85c8-ppgw-ccpr
- https://github.com/advisories/GHSA-68fv-2mgg-jv7q

## Changelog

- **2026-10-14 — Rate-limit backoff fix (image jobs).** A provider 429 no longer burns retries back-to-back: failed attempts back off (`2s × attempt`) before re-queueing, and an exhausted failover chain backs off the same way ([jobs.rs](../src-tauri/src/jobs.rs) `finish_image_error`). As part of the same rate-limit handling, a `cloud.gemini` 429 now fails over to the offline `local.a1111` provider within the same attempt, with promotion recording the provider that actually generated the output ([image_generation.rs](../src-tauri/src/image_generation.rs) `FailoverCommandExecutor`).
