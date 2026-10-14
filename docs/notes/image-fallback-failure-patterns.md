# Image-fallback failure-fix patterns

Regression notes for the image-generation fallback wave (commit
`fix(generation): finish image fallback and unbreak migration/image tests`).
Three failure patterns broke image jobs or burned their retry budget in the
field. Each is recorded here with its fix contract and the regression guard
that pins it. If a guard below goes red, a fix from this wave has regressed —
restore the contract instead of deleting the test.

Run the guards with:

```bash
npm run test:rust   # cargo test --lib through scripts/fix_test_manifest.py
```

## Pattern 1 — Configured Gemini image model not available to the API key

**Failure.** Cloud image jobs failed outright when the configured
`gemini_model` was absent from the key's `Models.list`, or listed with only
`countTokens` support. Model availability varies per key and per rollout, so
any single pinned model name left some keys unable to generate at all.

**Fix contract** (`resolve_cloud_image_model` in
[src-tauri/src/image_generation.rs](../../src-tauri/src/image_generation.rs)):
probe `Models.list`, require `supportedGenerationMethods` to contain
`generateContent`, prefer the configured model when it qualifies, otherwise
walk `CLOUD_IMAGE_MODEL_FALLBACKS` **in order**. If nothing qualifies, fail
with an actionable `InvalidConfig` ("enable Gemini image generation for the
key in Google AI Studio") instead of a mid-job HTTP error.

**Regression guards** (same file):
- `cloud_model_selection_prefers_configured_available_model_and_requires_generation_method`
- `cloud_model_fallback_skips_candidates_without_generate_content` — pins the
  skip rule (countTokens-only candidates are never selected) and fallback
  order.

## Pattern 2 — Permanent cloud rejections burned retries; transient ones lost their reason

**Failure.** Permanent cloud errors (403/404 "model not available to this
key") were retried like transient ones and burned the whole attempt budget,
and Google's error reason was swallowed, so users saw an opaque failure. The
mirror-image bug is just as damaging: classifying everything permanent
strands recoverable jobs on a single 429/5xx blip.

**Fix contract** (`classify_cloud_status`, same file): 5xx, 408 and 429 map
to `ImageError::Server(code)` (**retryable**); every other status maps to
`ImageError::CloudRejected { status, message }` (**permanent**), keeping
Google's reason verbatim but bounded (8 KiB error-body cap, 500-char
message) so a lying or oversized error body cannot poison classification.

**Regression guards:**
- `cloud_errors_preserve_google_reason_without_retrying_permanent_404s`
- `cloud_client_errors_are_permanent_but_transient_statuses_stay_retryable` —
  pins the full status split in both directions.
- `http_status_and_transport_classes_control_retry` — the local-provider side
  of the same retry table.

## Pattern 3 — Auto-retry loop reproduced the same framing and burned attempts instantly

**Failure (observed live).** A Gemini 429 burned all five attempts within one
second because auto-retry re-queued instantly. Separately, a crop rejection
requeued the payload unchanged — but the seed is baked into the stored
payload, so the next attempt reproduced the *identical* framing forever.

**Fix contract** (`finish_image_error` plus the composition gate in
[src-tauri/src/jobs.rs](../../src-tauri/src/jobs.rs)):

| Error | Retry? | Payload on requeue |
|---|---|---|
| `Server(429)` | yes, after `2s × attempt_count` backoff (sleeps outside the DB lock) | unchanged |
| `CroppedComposition` | yes | **re-seeded + CFG escalated** (+2.5 per retry, capped at 13) |
| other `retryable()` (Timeout, other Server, Unavailable) | yes, instantly | unchanged |
| non-retryable (Rejected, Malformed, CloudRejected, …) | no — `IMAGE_PERMANENT` | — |

`needs_reseed()` is true **only** for `CroppedComposition`. The composition
gate repairs first (`source_normalization::reframe_with_margin` recomposes a
isolatable subject onto a margined canvas); an unrepairable output with
attempts left is rejected as `CroppedComposition` and requeues re-seeded. On
the final attempt the gate does **not** fail the job: the output reaches the
human checkpoint (Hunyuan generator → "Regenerate Concept") instead of being
silently promoted or silently failed.

**Regression guards** (src-tauri/src/jobs.rs):
- `rate_limit_429_backs_off_before_the_next_auto_retry` — pins the 429
  backoff and that non-429 transient errors stay instant.
- `crop_rejection_reseeds_and_escalates_guidance_only_when_needed` — pins the
  re-seed + CFG escalation + the 13 cap, and that non-crop errors keep the
  stored payload untouched.

## Known gaps / open discrepancy

- **The final-attempt gate has no automated guard.** The composition gate
  lives inline in the image worker closure, and the full-flow image tests are
  `#[ignore]`d real-runtime tests
  (`real_a1111_generation_persists_managed_asset`). The final-attempt "never
  fail the job, surface the human checkpoint" behavior is therefore
  documented-but-unguarded; a scripted-loopback worker test is the right
  long-term guard.
- **Contradictory inline comments.** Two comment blocks above the gate in
  jobs.rs claim the final attempt "fails loudly" / "rejects permanently",
  while the third comment, the code itself, and the shipped "Regenerate
  Concept" UI all implement never-fail. The enforced behavior is the one this
  document describes; reconcile the stale comments before relying on them.
