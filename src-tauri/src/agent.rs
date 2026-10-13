//! The Nexora studio agent: turns one natural-language prompt into an
//! executable plan for the chat dashboard.
//!
//! Primary path: the configured LLM (OpenRouter/OpenAI/Anthropic/Gemini)
//! classifies intent and extracts generation parameters as JSON.
//! Fallback path: deterministic keyword heuristics, so the chat dashboard
//! keeps working with no API key configured or the network down.

use serde::{Deserialize, Serialize};

use crate::llm::{self, ChatMessage, ChatRequest, MessageRole};

pub const INTENTS: [&str; 7] = ["image", "video", "model3d", "full_pipeline", "chat", "asset_pack", "code"];
pub const QUALITIES: [&str; 3] = ["draft", "standard", "high"];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentPlan {
    /// One of `INTENTS`.
    pub intent: String,
    /// One-sentence description of what the agent will do.
    pub summary: String,
    /// Optimized English generation prompt for the target model.
    pub subject: String,
    pub negative_prompt: Option<String>,
    /// One of `QUALITIES`.
    pub quality: String,
    /// How many variants to generate (images only), 1-4.
    pub count: u32,
    /// Optional multi-step chain the agent executes autonomously, e.g.
    /// ["image", "model3d"] for an asset_pack: concept -> 3D -> approve -> Unity.
    #[serde(default)]
    pub steps: Vec<String>,
    /// For `code` plans: the target .cs file name (e.g. "CarController.cs").
    #[serde(default)]
    pub file_name: Option<String>,
    /// `llm` or `heuristic`.
    pub source: String,
    /// Provider that actually generated this plan (e.g. "gemini").
    /// `None` for heuristic fallbacks — the UI shows no badge then.
    #[serde(default)]
    pub plan_provider: Option<String>,
    /// Model that actually generated this plan (e.g. "openai/gpt-oss-20b").
    #[serde(default)]
    pub plan_model: Option<String>,
    /// True when the plan came from a provider other than the active one
    /// (auto-switch kicked in after a quota hit / network failure).
    #[serde(default)]
    pub plan_is_fallback: bool,
}

const SYSTEM_PROMPT: &str = r#"You are the Nexora Studio agent — an all-rounder app-building AI inside a production studio app. It can create games AND any other application (desktop, web, mobile) by generating assets and writing code for any platform/language.
LANGUAGE RULE: the user may write in ANY language (English, Urdu, Roman Urdu, Arabic, Spanish, Hindi, Chinese, ...). Detect the language the user wrote in and write the "summary" field in THAT SAME language. The "subject" field must ALWAYS be a polished English generation prompt optimized for image/video/3D models, regardless of the user's language.
Classify the user's request into EXACTLY ONE intent:
- "image": the user wants only a 2D image / picture / concept art / texture.
- "video": the user wants only a video / animation / clip.
- "model3d": the user explicitly wants a 3D model generated directly (text-to-3D) with no concept stage.
- "full_pipeline": the user wants a complete, game-ready 3D asset (concept image -> 3D generation -> optimization). This is the default whenever they talk about making an asset/model/car/character/prop for their game.
- "asset_pack": the user wants a COMPLETE game-ready asset pack produced AUTONOMOUSLY from one subject — e.g. 'make me a full asset pack of this car', 'complete game-ready pack'. The agent will run every step itself: concept image -> approve -> 3D generation -> optimization -> approve -> Unity deploy. Use it when the user asks for the whole thing end-to-end, not just one asset. You may add an optional "steps" array to customize the chain, e.g. ["image","video","model3d"] — valid step intents are image, video, model3d (no chat, max 4 steps).
- "code": the user wants code written or edited for ANY platform — Unity/C#, desktop apps, web apps, mobile apps (Flutter/Dart, Kotlin, Swift), scripts, tools. Set "subject" to a precise English description of what the code must do, and set "fileName" to a sensible relative path with the RIGHT extension for the language (e.g. "CarController.cs", "src/main.py", "lib/main.dart", "src/app.ts", "index.html"). If the user names an existing file to edit, use that exact path.
- "chat": a general question, advice, or anything with no asset to create.
Reply with ONLY a JSON object, no markdown, no commentary:
{"intent":"image|video|model3d|full_pipeline|asset_pack|code|chat","subject":"<one-line English generation prompt describing the subject, optimized for the target generator>","negativePrompt":"<short comma-separated list or empty string>","quality":"draft|standard|high","count":1,"steps":[],"fileName":null,"summary":"<one short sentence in the USER'S language describing what you will do>"}"#;

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn strip_fillers(prompt: &str) -> String {
    let fillers = [
        "please", "sirf", "only", "just", "make", "create", "generate", "bana do", "banado",
        "banao", "bana", "chahiye", "mujhe", "kar do", "kardo", "ek ", "a ", "an ", "me ", "mujhay",
    ];
    let mut cleaned = prompt.trim().to_string();
    let lower = cleaned.to_lowercase();
    for filler in fillers {
        if lower.starts_with(filler) {
            let cut = filler.len();
            if cleaned.len() > cut {
                cleaned = cleaned[cut..].trim().to_string();
            }
        }
    }
    if cleaned.is_empty() {
        prompt.trim().to_string()
    } else {
        cleaned
    }
}

/// Deterministic fallback planner — no network, no API key required.
pub fn heuristic_plan(prompt: &str) -> AgentPlan {
    let lower = prompt.to_lowercase();
    let wants_video = contains_any(&lower, &["video", "animation", "animate", "clip", "movie"]);
    let wants_image = contains_any(
        &lower,
        &["image", "picture", "photo", "concept art", "poster", "texture", "2d"],
    );
    // Words that strongly imply a game-ready 3D asset (full-pipeline target).
    let strong_3d = contains_any(
        &lower,
        &[
            "3d", "model", "mesh", "glb", "asset", "game asset", "character model",
        ],
    );
    let wants_create = contains_any(
        &lower,
        &["make", "create", "generate", "banao", "banado", "bana do", "chahiye"],
    );
    let only_marker = contains_any(&lower, &["only", "sirf", "just", "solely"]);
    let direct_3d = contains_any(&lower, &["direct 3d", "text to 3d", "text-to-3d"]);
    // "Complete game-ready asset pack" style requests: the agent runs every
    // step autonomously (image -> approve -> 3D -> approve -> Unity).
    let wants_pack = contains_any(
        &lower,
        &["asset pack", "pack", "poora", "pura", "complete", "sab kuch", "full set", "game ready set", "end to end"],
    );
    // Code requests for ANY platform: "car controller script banao",
    // "fix the bug in my project", "mera flutter project theek karo",
    // "mobile app banao", "website banao" — app/website words now count as
    // code intent (the earlier list only knew C#/Unity keywords, so app
    // requests fell through to the asset pipeline).
    let wants_code = contains_any(
        &lower,
        &["script", "c#", "csharp", "monobehaviour", "update method", "code likho", "code banao", "code likh", "bug", "error", "issue", "warning", "crash", "exception", "app banao", "app bana", "application banao", "application bana", "mobile app", "web app", "website", "web site", "flutter", "kotlin", "swift", "react", "html", "python program", "desktop app", "extension", "plugin", "software", "tool likho", "tool banao"],
    );
    let wants_code_edit = contains_any(
        &lower,
        &["likho", "write", "edit", "fix", "update", "refactor", "theek", "optimize the code", "likh", "solve", "resolve"],
    );
    let starts_question = ["how", "what", "why", "kaise", "kya", "kyun"]
        .iter()
        .any(|q| lower.starts_with(q));
    // Roman-Urdu question markers anywhere in the sentence ("mujhy yeh batao k...",
    // "... match karti he k ni") — these must win over the "3d" keyword.
    let asks_in_roman_urdu = contains_any(
        &lower,
        &["batao", "bata do", "bata de", "btao", "batayen", "k ni", "k nahi", "k nai", "hai ya nahi", "mujhe batao", "mujhy batao", "bata sakte"],
    );
    let looks_like_question = (lower.contains('?') && !wants_create)
        || (starts_question && !wants_create)
        || (asks_in_roman_urdu && !wants_create)
        || contains_any(&lower, &["explain", "mujhe batao"]);
    // Greetings and small talk ("hi", "hello there", "assalam o alaikum") must
    // never trigger an asset pipeline — route them to chat.
    let normalized = lower.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    let is_greeting = lower.split_whitespace().count() <= 3
        && ["hi", "hello", "hey", "yo", "sup", "hola", "salam", "assalam", "asalam", "aoa", "namaste", "greetings"]
            .iter()
            .any(|g| normalized == *g || normalized.starts_with(&format!("{g} ")));

    // Priority: an explicit "sirf/only" scope wins, then the media type the
    // prompt names, then greetings/questions, then 3D asset work, then — only
    // for explicit creation requests — the full pipeline. Anything else that
    // looks like conversation stays in chat instead of spawning jobs.
    let intent = if is_greeting {
        "chat"
    } else if wants_image && only_marker {
        "image"
    } else if wants_video && only_marker {
        "video"
    } else if wants_video && !wants_image {
        "video"
    } else if wants_image {
        "image"
      // Explicit app/website/software words WIN over asset keywords —
      // "mobile app banao jis me alarm ho" is code, not a 3D pipeline.
    } else if wants_code && (wants_create || wants_code_edit) {
        "code"
    } else if looks_like_question && !wants_create {
        "chat"
    } else if wants_pack && wants_create {
        "asset_pack"
    } else if strong_3d {
        "full_pipeline"
    } else if direct_3d {
        "model3d"
    } else if wants_create {
        "full_pipeline"
    } else {
        "chat"
    };

    let quality = if contains_any(
        &lower,
        &["aaa", "best", "highest", "high quality", "game ready", "game-ready", "premium", "top quality"],
    ) {
        "high"
    } else if contains_any(&lower, &["draft", "quick", "fast", "test", "rough", "jaldi"]) {
        "draft"
    } else if intent == "full_pipeline" || intent == "model3d" {
        "high"
    } else {
        "standard"
    };

    let subject = strip_fillers(prompt);
    let summary = match intent {
        "image" => format!("I will generate a single image: \"{subject}\" ({quality})."),
        "video" => format!("I will generate a single video: \"{subject}\" ({quality})."),
        "model3d" => format!("I will generate a direct 3D model (text-to-3D): \"{subject}\" ({quality})."),
        "chat" => "This is a question — I will do my best to answer it.".to_string(),
        "asset_pack" => format!(
            "Complete asset pack, run fully autonomously: concept image → approve → 3D generation → optimization → approve → Unity deploy. Subject: \"{subject}\" ({quality})."
        ),
        "code" => format!(
            "I will write complete code for the requested platform (Unity/C#, Flutter/Dart, Kotlin, Swift, web, Python, desktop — whatever the request names): \"{subject}\". It saves into your workspace when you approve."
        ),
        _ => format!(
            "Full pipeline: concept image → 3D generation → optimization, then approve and send to Unity. Subject: \"{subject}\" ({quality})."
        ),
    };

    let steps: Vec<String> = if intent == "asset_pack" {
        if wants_video {
            vec!["image".into(), "video".into(), "model3d".into()]
        } else {
            vec!["image".into(), "model3d".into()]
        }
    } else {
        Vec::new()
    };

    AgentPlan {
        intent: intent.to_string(),
        summary,
        subject,
        negative_prompt: Some(
            "blurry, ugly, deformed, low quality, artifacts, distorted, noisy, watermark".into(),
        ),
        quality: quality.to_string(),
        count: 1,
        steps,
        file_name: None,
        source: "heuristic".into(),
        plan_provider: None,
        plan_model: None,
        plan_is_fallback: false,
    }
}

fn parse_plan_json(text: &str) -> Option<AgentPlan> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    let value: serde_json::Value = serde_json::from_str(&text[start..=end]).ok()?;

    let intent = value.get("intent")?.as_str()?.trim().to_lowercase();
    if !INTENTS.contains(&intent.as_str()) {
        return None;
    }
    let subject = value
        .get("subject")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())?
        .to_string();
    let quality = value
        .get("quality")
        .and_then(|v| v.as_str())
        .map(|q| q.trim().to_lowercase())
        .filter(|q| QUALITIES.contains(&q.as_str()))
        .unwrap_or_else(|| if intent == "image" || intent == "video" { "standard".into() } else { "high".into() });
    let count = value
        .get("count")
        .and_then(|v| v.as_u64())
        .map(|c| c.clamp(1, 4) as u32)
        .unwrap_or(1);
    let negative_prompt = value
        .get("negativePrompt")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let summary = value
        .get("summary")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("Plan: {intent} — {subject}"));

    let mut steps: Vec<String> = value
        .get("steps")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| s.as_str())
                .map(|s| s.trim().to_lowercase())
                .filter(|s| {
                    matches!(s.as_str(), "image" | "video" | "model3d")
                })
                .take(4)
                .collect()
        })
        .unwrap_or_default();
    if intent == "asset_pack" && steps.is_empty() {
        steps = vec!["image".to_string(), "model3d".to_string()];
    }

    let file_name = value
        .get("fileName")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    Some(AgentPlan {
        intent,
        summary,
        subject,
        negative_prompt,
        quality,
        count,
        steps,
        file_name,
        source: "llm".into(),
        plan_provider: None,
        plan_model: None,
        plan_is_fallback: false,
    })
}

/// Compact a conversation history for LLM context: keep only the most recent
/// `max_messages` entries and cap each message's length so long sessions stay
/// within token budgets.
pub fn trim_history(history: &[ChatMessage], max_messages: usize, max_chars: usize) -> Vec<ChatMessage> {
    history
        .iter()
        .rev()
        .take(max_messages)
        .rev()
        .map(|m| ChatMessage {
            role: m.role.clone(),
            content: {
                let chars: Vec<char> = m.content.chars().collect();
                if chars.len() > max_chars {
                    format!("{}…", chars[..max_chars].iter().collect::<String>())
                } else {
                    m.content.clone()
                }
            },
        })
        .collect()
}

/// Ask the configured LLM to interpret the prompt, with recent conversation
/// context so follow-ups ("ab is car ka 3D banao") resolve correctly.
///
/// Tries the active provider first; on transient failures (rate limit / 503 /
/// network / timeout) it walks the other configured providers before giving up.
/// Returns `None` only when every provider fails or the JSON is unparseable.
pub async fn llm_plan(
    config: &llm::LlmConfig,
    prompt: &str,
    history: &[ChatMessage],
) -> Option<AgentPlan> {
    let mut messages = vec![ChatMessage {
        role: MessageRole::System,
        content: SYSTEM_PROMPT.to_string(),
    }];
    messages.extend(trim_history(history, 16, 600));
    messages.push(ChatMessage {
        role: MessageRole::User,
        content: prompt.to_string(),
    });
    let request = ChatRequest {
        messages,
        max_tokens: Some(400),
        temperature: Some(0.2),
    };

    // Try every configured provider in ranked order: active first, then
    // free-tier + capability ranked alternatives. The ranked list also swaps
    // in each provider's best free-tier model automatically, so a quota hit
    // on one model lands on the best available replacement.
    //
    // When the user disabled auto-switching, the chain is empty — fall back
    // to a single-provider try (active only, its own saved model).
    let fallback_order = llm::ranked_fallback_configs(config);
    let single_provider: Vec<llm::LlmProviderConfig> = if fallback_order.is_empty() {
        config.get_active().cloned().into_iter().collect()
    } else {
        Vec::new()
    };
    let chain: &[llm::LlmProviderConfig] = if fallback_order.is_empty() {
        &single_provider
    } else {
        &fallback_order
    };

    // The first entry of the ranked chain is the active provider; anything
    // past it is an auto-switch fallback worth surfacing in the UI.
    let active_pid = config.active_provider.clone();
    let mut first_try = true;
    for provider_cfg in chain {
        let trial_config = llm::LlmConfig {
            active_provider: provider_cfg.provider.clone(),
            providers: {
                let mut m = std::collections::HashMap::new();
                m.insert(provider_cfg.provider.clone(), provider_cfg.clone());
                m
            },
            // Irrelevant here — the trial already picked its provider.
            auto_switch_on_quota_hit: false,
        };
        let response = match llm::chat(&trial_config, request.clone()).await {
            Ok(resp) => resp,
            Err(err) => {
                // Transient failures are worth retrying on another provider.
                // Permanent auth / config problems are not — skip to the next.
                let transient = matches!(err.error_type.as_str(), "rate_limit" | "network");
                tracing::warn!(
                    "llm_plan provider failed: provider={provider} error_type={error_type} retry={retry}",
                    provider = provider_cfg.provider.to_string(),
                    error_type = err.error_type,
                    retry = transient,
                );
                if !transient {
                    // Auth or config issue — this provider won't work, skip.
                    continue;
                }
                // Rate limit / network glitch — fall through to next provider.
                continue;
            }
        };
        if let Some(mut plan) = parse_plan_json(&response.content) {
            plan.plan_provider = Some(provider_cfg.provider.to_string());
            plan.plan_model = Some(response.model);
            plan.plan_is_fallback = !first_try && provider_cfg.provider != active_pid;
            return Some(plan);
        }
        first_try = false;
    }
    None
}

/// Full interpret path with fallback. Never fails for non-empty prompts.
pub async fn interpret(config: &llm::LlmConfig, prompt: &str, history: &[ChatMessage]) -> AgentPlan {
    if let Some(plan) = llm_plan(config, prompt, history).await {
        return plan;
    }
    heuristic_plan(prompt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_keywords_route_to_video() {
        let plan = heuristic_plan("make a short video of a racing car on neon highway");
        assert_eq!(plan.intent, "video");
        assert_eq!(plan.quality, "standard");
    }

    #[test]
    fn image_only_prompts_route_to_image() {
        let plan = heuristic_plan("sirf ek picture banao neon car ka concept art");
        assert_eq!(plan.intent, "image");
    }

    #[test]
    fn three_d_assets_default_to_full_pipeline_at_high_quality() {
        let plan = heuristic_plan("make a 3d car asset for my game, AAA quality");
        assert_eq!(plan.intent, "full_pipeline");
        assert_eq!(plan.quality, "high");
    }

    #[test]
    fn draft_keywords_lower_quality() {
        let plan = heuristic_plan("quick draft test of a 3d prop");
        assert_eq!(plan.quality, "draft");
    }

    #[test]
    fn questions_route_to_chat() {
        let plan = heuristic_plan("how do I set up a Unity project for my assets?");
        assert_eq!(plan.intent, "chat");
    }

    #[test]
    fn greetings_route_to_chat_never_pipeline() {
        for text in ["HI", "hi", "Hello!", "hey there", "assalam o alaikum", "Salam bhai"] {
            let plan = heuristic_plan(text);
            assert_eq!(plan.intent, "chat", "greeting {text:?} must route to chat");
        }
    }

    #[test]
    fn roman_urdu_questions_beat_3d_keyword() {
        let plan = heuristic_plan(
            "mujhy yeh batao k es nexora gaming studio ki 3d quality AAA class games ko match karti he k ni",
        );
        assert_eq!(plan.intent, "chat");
    }

    #[test]
    fn unknown_conversation_defaults_to_chat_not_pipeline() {
        let plan = heuristic_plan("tum kaise ho");
        assert_eq!(plan.intent, "chat");
    }

    #[test]
    fn explicit_create_still_runs_pipeline() {
        let plan = heuristic_plan("Make a neon sports car, AAA quality");
        assert_eq!(plan.intent, "full_pipeline");
    }

    #[test]
    fn asset_pack_requests_get_autonomous_chain() {
        let plan = heuristic_plan("is car ka complete game-ready asset pack banao");
        assert_eq!(plan.intent, "asset_pack");
        assert_eq!(plan.steps, vec!["image".to_string(), "model3d".to_string()]);
    }

    #[test]
    fn trim_history_keeps_recent_and_caps_length() {
        let history = (0..30)
            .map(|i| ChatMessage {
                role: if i % 2 == 0 { MessageRole::User } else { MessageRole::Assistant },
                content: format!("msg {:02} {}", i, "x".repeat(1000)),
            })
            .collect::<Vec<_>>();
        let trimmed = trim_history(&history, 10, 40);
        assert_eq!(trimmed.len(), 10);
        // Keeps the most recent messages in original order.
        assert!(trimmed[0].content.starts_with("msg 20"));
        assert!(trimmed[9].content.starts_with("msg 29"));
        // Caps per-message length.
        assert!(trimmed[0].content.chars().count() <= 41);
    }

    #[test]
    fn code_requests_route_to_code_intent() {
        let plan = heuristic_plan("car controller script banao with drift");
        assert_eq!(plan.intent, "code");
    }

    #[test]
    fn bug_fix_requests_route_to_code_intent() {
        let plan = heuristic_plan("mera project error de raha hai, fix karo");
        assert_eq!(plan.intent, "code");
        let plan = heuristic_plan("fix the bug in my flutter app");
        assert_eq!(plan.intent, "code");
    }

    #[test]
    fn questions_about_errors_stay_chat() {
        let plan = heuristic_plan("what does this error mean?");
        assert_eq!(plan.intent, "chat");
    }

    #[test]
    fn parse_plan_json_reads_file_name() {
        let plan = parse_plan_json(
            r#"{"intent":"code","subject":"drift car controller","quality":"high","count":1,"fileName":"CarController.cs","summary":"Write script"}"#,
        )
        .expect("valid code plan must parse");
        assert_eq!(plan.intent, "code");
        assert_eq!(plan.file_name.as_deref(), Some("CarController.cs"));
    }

    #[test]
    fn parse_plan_json_reads_custom_steps() {
        let plan = parse_plan_json(
            r#"{"intent":"asset_pack","subject":"rugged sci-fi crate","quality":"high","count":1,"steps":["image","video","model3d"],"summary":"Full pack"}"#,
        )
        .expect("valid asset_pack plan must parse");
        assert_eq!(plan.steps, vec!["image".to_string(), "video".to_string(), "model3d".to_string()]);
    }

    #[test]
    fn json_parsing_accepts_valid_plan() {
        let plan = parse_plan_json(
            r#"```json
            {"intent":"image","subject":"neon sports car","negativePrompt":"blurry","quality":"high","count":2,"summary":"Two images"}
            ```"#,
        )
        .unwrap();
        assert_eq!(plan.intent, "image");
        assert_eq!(plan.count, 2);
        assert_eq!(plan.quality, "high");
        assert_eq!(plan.source, "llm");
    }

    #[test]
    fn json_parsing_rejects_bad_intent_and_missing_subject() {
        assert!(parse_plan_json(r#"{"intent":"fly","subject":"x"}"#).is_none());
        assert!(parse_plan_json(r#"{"intent":"image"}"#).is_none());
        assert!(parse_plan_json("not json at all").is_none());
    }

    #[test]
    fn count_is_clamped() {
        let plan = parse_plan_json(r#"{"intent":"image","subject":"x","count":99}"#).unwrap();
        assert_eq!(plan.count, 4);
    }
}
