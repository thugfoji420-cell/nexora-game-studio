# Rill walkthrough — Studio Chat history flow

**App:** Nexora Game Studio (Tauri 2 desktop app — it has no hosted URL)
**Flow:** Ask the Studio Chat agent a greeting and show that the conversation is saved and restored after a restart.
**Test data:** the message `hello` (Roman-Urdu variant: `kia hal`).

## Numbered steps

1. Launch the Nexora desktop app and open the **Dashboard (Studio Chat)** — the screen headed "What shall we build?".
2. In the message box (placeholder "Chat with me, ask for code, or type: 'Make a neon sports car, AAA quality'...") type exactly: `hello`
3. Press **Enter** (or click the **Send** button).
4. Wait for the agent's reply bubble to appear.
5. Fully quit the app, relaunch it, and open the Dashboard again.
6. Click **Clear history** (top right) and confirm the "Clear saved chat history?" dialog.

## Expected behaviour

- **Steps 3–4** — `hello` appears as a user bubble with a timestamp; the agent replies instantly with no model call (built-in small talk): "I'm doing great, thanks! 😊 What are we building today — image, video, 3D, or code?". A Roman-Urdu greeting such as `kia hal` is answered in Roman Urdu.
- **Step 5** — both messages come back, reloaded from saved history and ordered by time, proving the conversation survives a restart. History is stored per mode (`agent`, plus legacy `chat` rows), so nothing is lost on relaunch.
- **Step 6** — after confirming, the saved history is cleared and the thread resets to the welcome message only.

## Missing inputs

- **Public app URL** — none was supplied, and a Tauri desktop app has no hosted URL, so this walkthrough targets the local desktop build only. The browser design preview disables saved history. Add a public app URL in Rill before running; a local/private target cannot be reached from the cloud.
- **AI provider** — not needed for this flow (the greeting reply is produced locally). A configured provider (Gemini key or the local offline engine) is required only for real, non-greeting answers.

## Handoff

Cloud runs cannot reach a local desktop app. For a manual screen recording, use https://userill.dev/record?utm_source=freebuff&utm_medium=paid&utm_campaign=rill — or, to record with an existing browser agent, https://userill.dev/docs/agent-quickstart?utm_source=freebuff&utm_medium=paid&utm_campaign=rill

[Open your walkthrough in Rill](https://userill.dev/freebuff?utm_source=freebuff&utm_medium=paid&utm_campaign=rill&utm_content=text-ad&bfcid=bfc_1.eyJpZCI6ImJmZDllYmEyLTBmMTktNGIzMC04OWI1LWZmNzljMTUyZWRjMSIsImMiOiIxNDQyMTUzOS1kYjNiLTQ1YzEtYjQ5OC1hYTMxOTFmNDkxYzAiLCJrIjoiNDU4ODZlNWEtYzkyZC00ZDUxLTg1ZjgtYzc4YTRmMTY3ZWYwIiwiaWF0IjoxNzkxODEzODAzfQ.x3S6b90dUfNfSyZGi1vr2AcJeKej10fuCP0sLBLH2zE#task=Nexora%20Game%20Studio%20%E2%80%94%20Studio%20Chat%20history%20flow%20%28Tauri%20desktop%20app%3B%20it%20has%20no%20hosted%20URL%29%0A%0A1.%20Launch%20the%20Nexora%20desktop%20app%20and%20open%20the%20Dashboard%20%28Studio%20Chat%29%3A%20the%20screen%20headed%20%22What%20shall%20we%20build%3F%22.%0A2.%20In%20the%20message%20box%20%28placeholder%20%22Chat%20with%20me%2C%20ask%20for%20code%2C%20or%20type%3A%20%27Make%20a%20neon%20sports%20car%2C%20AAA%20quality%27...%22%29%20type%20exactly%3A%20hello%0A3.%20Press%20Enter%20%28or%20click%20the%20Send%20button%29.%0A4.%20Wait%20for%20the%20agent%27s%20reply%20bubble%20to%20appear.%0A5.%20Fully%20quit%20the%20app%2C%20relaunch%20it%2C%20and%20open%20the%20Dashboard%20again.%0A6.%20Click%20%22Clear%20history%22%20%28top%20right%29%20and%20confirm%20the%20%22Clear%20saved%20chat%20history%3F%22%20dialog.%0A%0AExpected%20behaviour%3A%0A-%20After%20steps%203-4%3A%20%22hello%22%20shows%20as%20a%20user%20bubble%20with%20a%20timestamp%2C%20and%20the%20agent%20answers%20instantly%20with%20no%20model%20call%20%28built-in%20small%20talk%29%3A%20%22I%27m%20doing%20great%2C%20thanks!%20%F0%9F%98%8A%20What%20are%20we%20building%20today%20%E2%80%94%20image%2C%20video%2C%203D%2C%20or%20code%3F%22%20%28a%20Roman-Urdu%20greeting%20like%20%22kia%20hal%22%20replies%20in%20Roman%20Urdu%29.%0A-%20After%20step%205%3A%20both%20messages%20come%20back%2C%20reloaded%20from%20saved%20history%20and%20ordered%20by%20time%20%E2%80%94%20proving%20chat%20history%20survives%20a%20restart.%0A-%20After%20step%206%3A%20the%20thread%20resets%20to%20the%20welcome%20message%20only.%0A%0AMissing%20input%3A%20no%20public%20app%20URL%20was%20supplied%2C%20so%20this%20runs%20on%20the%20local%20desktop%20build%20only%20%28the%20browser%20preview%20disables%20saved%20history%29.%20A%20configured%20AI%20provider%20is%20not%20needed%20for%20this%20flow.)

Your walkthrough is prepared. No recording has been made yet. In Rill, review the steps, add a public app URL, and choose Sign up to run if you need an account. The walkthrough uses Interact mode. Press Run to start it.
