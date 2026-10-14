# Nexora Game Studio v0.2.10 — tooling checkpoint

_Draft for review — 2026-10-14. Covers the four new commits `6339a18`…`fd0bf5b`._

This release hardens the image pipeline against rate limits, surfaces real
job failures in chat with a one-click retry, and moves the provider test
suite onto shared tooling.

## Highlights

- **429s no longer burn attempts** — when `cloud.gemini` rate-limits, the
  same attempt immediately falls back to the offline `local.a1111` provider
  instead of waiting out the quota window, and exhausted chains back off
  `2s × attempt` before re-queueing.
- **Real errors in chat, with Retry now** — image error bubbles show the
  actual backend error (`error_code: error_message`) instead of a generic
  "image job failed", and a Retry now button re-queues the failed job when
  eligible.
- **Shared mock HTTP server for tests** — ~300 lines of duplicated socket
  plumbing across four test modules replaced by one std-only
  `test-support` crate; tests that never reach the endpoint now fail fast
  instead of hanging.

## Changes

### feat(jobs): image-job 429 failover to offline A1111 with rate-limit backoff — `0fda20b`

- `FailoverCommandExecutor` runs the offline `local.a1111` command in the
  **same attempt** on a 429; non-429 errors stay terminal and verbatim.
- The fallback request never carries the Gemini key, and promotion records
  the payload of the provider that actually generated the output.
- Exhausted failover chains fail retryable with the full per-provider
  trail; `finish_image_error` backs off `2s × attempt` on both bare 429s
  and exhausted chains.
- New failure-pattern notes
  ([docs/notes/image-fallback-failure-patterns.md](notes/image-fallback-failure-patterns.md))
  with regression guards for fallback order, the permanent/transient
  split, and crop-reseed + CFG escalation.

### feat(chat): real image-job errors and Retry now in the chat error bubble — `bcf4d49`

- Error bubbles (Agent, Midjourney and the outer fallback) resolve the
  job's real backend error via `get_job_details`.
- Error cards carry a retry context (exact plan + failed job id, persisted
  in chat history); **Retry now** re-queues through the real `retry_job`
  when eligible, falling back to a fresh job otherwise, restoring the ready
  card on success.
- Three new tests pin the error-message mapping.

### test(support): shared mock HTTP server crate for provider tests — `6339a18`

- New std-only `test-support` dev-dependency exposing a handler-based
  `MockHttpServer` with Content-Length-aware request parsing, canned
  response builders and accept deadlines.
- The code_agent scripted server and all ComfyUI stubs migrate onto it;
  the image/jobs test modules follow in `0fda20b`.

### docs: changelog the rate-limit backoff fix — `fd0bf5b`

- Dated changelog entries in `README.md` and
  `docs/application-audit.md` covering the 429 backoff fix and the
  same-attempt offline failover it ships with.

## Verification

`npm run ci` green end-to-end on the tagged tree: typecheck ✓ · 117 JS
tests (16 files) ✓ · 264 Rust tests (0 failed, 2 ignored) ✓ ·
`vite build` ✓ · `cargo check` ✓.

## Also included in this tag

- `82dac29` chore(repo): pins line endings with `.gitattributes`, ending
  the phantom line-ending diffs in `git status`.
