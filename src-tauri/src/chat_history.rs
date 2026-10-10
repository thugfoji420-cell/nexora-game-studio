//! Persistent Dashboard chat history (SQLite).
//!
//! Stores Chat/Agent conversation turns in a small local database so the
//! conversation survives app restarts. One table, per-mode rows
//! (`agent` = Asset Agent mode, `chat` = Chat mode). The optional `card`
//! column keeps the serialized result card shown under agent messages.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const MODE_AGENT: &str = "agent";
pub const MODE_CHAT: &str = "chat";

const VALID_MODES: [&str; 2] = [MODE_AGENT, MODE_CHAT];
const VALID_ROLES: [&str; 3] = ["user", "agent", "assistant"];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatHistoryMessage {
    pub id: String,
    pub role: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub card: Option<serde_json::Value>,
    pub time: i64,
}

fn open(path: &Path) -> Result<Connection, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("failed to create db dir: {e}"))?;
    }
    let conn = Connection::open(path).map_err(|e| format!("failed to open chat db: {e}"))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS chat_history (
            id   TEXT PRIMARY KEY,
            mode TEXT NOT NULL,
            role TEXT NOT NULL,
            text TEXT NOT NULL,
            card TEXT,
            time INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_chat_history_mode_time
            ON chat_history(mode, time);",
    )
    .map_err(|e| format!("failed to init chat db: {e}"))?;
    Ok(conn)
}

fn validate_mode(mode: &str) -> Result<(), String> {
    if VALID_MODES.contains(&mode) {
        Ok(())
    } else {
        Err(format!("invalid chat history mode: {mode}"))
    }
}

fn validate_role(role: &str) -> Result<(), String> {
    if VALID_ROLES.contains(&role) {
        Ok(())
    } else {
        Err(format!("invalid chat history role: {role}"))
    }
}

/// Insert one message. Uses the caller-supplied `id` when given so the UI and
/// the database stay in sync; otherwise mints a UUIDv7.
pub fn append(
    path: &Path,
    id: Option<&str>,
    mode: &str,
    role: &str,
    text: &str,
    card: Option<&serde_json::Value>,
    time: i64,
) -> Result<ChatHistoryMessage, String> {
    validate_mode(mode)?;
    validate_role(role)?;
    let id = match id {
        Some(existing) if !existing.trim().is_empty() => existing.trim().to_string(),
        _ => uuid::Uuid::now_v7().to_string(),
    };
    let card_json = match card {
        Some(value) => Some(serde_json::to_string(value).map_err(|e| e.to_string())?),
        None => None,
    };
    let conn = open(path)?;
    conn.execute(
        "INSERT INTO chat_history (id, mode, role, text, card, time) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, mode, role, text, card_json, time],
    )
    .map_err(|e| format!("failed to insert chat message: {e}"))?;
    Ok(ChatHistoryMessage {
        id,
        role: role.to_string(),
        text: text.to_string(),
        card: card.cloned(),
        time,
    })
}

/// Load one mode's history ordered oldest-first.
pub fn load(path: &Path, mode: &str) -> Result<Vec<ChatHistoryMessage>, String> {
    validate_mode(mode)?;
    let conn = open(path)?;
    let mut stmt = conn
        .prepare("SELECT id, role, text, card, time FROM chat_history WHERE mode = ?1 ORDER BY time ASC, rowid ASC")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![mode], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut messages = Vec::new();
    for row in rows {
        let (id, role, text, card_json, time) = row.map_err(|e| e.to_string())?;
        validate_role(&role)?;
        let card = match card_json {
            Some(raw) => Some(
                serde_json::from_str(&raw).map_err(|e| format!("corrupt card payload: {e}"))?,
            ),
            None => None,
        };
        messages.push(ChatHistoryMessage {
            id,
            role,
            text,
            card,
            time,
        });
    }
    Ok(messages)
}

/// Patch a message's text and/or card. `None` = leave unchanged;
/// `Some(None)` for the card clears it; `Some(Some(value))` replaces it.
pub fn update(
    path: &Path,
    mode: &str,
    id: &str,
    text: Option<&str>,
    card: Option<Option<serde_json::Value>>,
) -> Result<(), String> {
    validate_mode(mode)?;
    if text.is_none() && card.is_none() {
        return Ok(());
    }
    let conn = open(path)?;
    let mut sets = Vec::new();
    let mut args: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    if let Some(t) = text {
        sets.push("text = ?".to_string());
        args.push(Box::new(t.to_string()));
    }
    if let Some(card_value) = card {
        sets.push("card = ?".to_string());
        match card_value {
            None => args.push(Box::new(rusqlite::types::Null)),
            Some(value) => {
                let json = serde_json::to_string(&value).map_err(|e| e.to_string())?;
                args.push(Box::new(json));
            }
        }
    }
    args.push(Box::new(id.to_string()));
    args.push(Box::new(mode.to_string()));
    let sql = format!(
        "UPDATE chat_history SET {} WHERE id = ? AND mode = ?",
        sets.join(", ")
    );
    let affected = conn
        .execute(
            &sql,
            rusqlite::params_from_iter(args.iter().map(|a| a.as_ref())),
        )
        .map_err(|e| format!("failed to update chat message: {e}"))?;
    if affected == 0 {
        return Err("chat message not found".into());
    }
    Ok(())
}

/// Delete one mode's entire history.
pub fn clear(path: &Path, mode: &str) -> Result<(), String> {
    validate_mode(mode)?;
    let conn = open(path)?;
    conn.execute("DELETE FROM chat_history WHERE mode = ?1", params![mode])
        .map_err(|e| format!("failed to clear chat history: {e}"))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tauri command wrappers
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn chat_history_load(
    state: tauri::State<'_, crate::AppState>,
    mode: String,
) -> Result<Vec<ChatHistoryMessage>, String> {
    load(&state.chat_history_path, &mode)
}

#[tauri::command]
pub fn chat_history_append(
    state: tauri::State<'_, crate::AppState>,
    mode: String,
    role: String,
    text: String,
    id: Option<String>,
    card: Option<serde_json::Value>,
    time: Option<i64>,
) -> Result<ChatHistoryMessage, String> {
    append(
        &state.chat_history_path,
        id.as_deref(),
        &mode,
        &role,
        &text,
        card.as_ref(),
        time.unwrap_or_else(|| chrono::Utc::now().timestamp_millis()),
    )
}

#[tauri::command]
pub fn chat_history_update(
    state: tauri::State<'_, crate::AppState>,
    mode: String,
    id: String,
    text: Option<String>,
    card: Option<Option<serde_json::Value>>,
) -> Result<(), String> {
    update(&state.chat_history_path, &mode, &id, text.as_deref(), card)
}

#[tauri::command]
pub fn chat_history_clear(state: tauri::State<'_, crate::AppState>, mode: String) -> Result<(), String> {
    clear(&state.chat_history_path, &mode)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("nexora_chat_history_{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("chat_history.db")
    }

    #[test]
    fn append_load_roundtrip_keeps_order_and_ids() {
        let path = temp_db("roundtrip");
        append(&path, None, MODE_AGENT, "user", "make a car", None, 100).unwrap();
        append(&path, Some("custom-id"), MODE_AGENT, "agent", "on it", None, 200).unwrap();
        append(&path, None, MODE_CHAT, "user", "hello", None, 50).unwrap();

        let agent = load(&path, MODE_AGENT).unwrap();
        assert_eq!(agent.len(), 2);
        assert_eq!(agent[0].text, "make a car");
        assert_eq!(agent[1].id, "custom-id");

        let chat = load(&path, MODE_CHAT).unwrap();
        assert_eq!(chat.len(), 1);
        assert_eq!(chat[0].text, "hello");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn card_survives_roundtrip_and_update_can_clear_it() {
        let path = temp_db("card");
        let card = serde_json::json!({ "kind": "image", "status": "ready" });
        append(&path, Some("m1"), MODE_AGENT, "agent", "done", Some(&card), 10).unwrap();
        let loaded = load(&path, MODE_AGENT).unwrap();
        assert_eq!(loaded[0].card.as_ref().unwrap()["kind"], "image");

        update(&path, MODE_AGENT, "m1", Some("final text"), None).unwrap();
        let loaded = load(&path, MODE_AGENT).unwrap();
        assert_eq!(loaded[0].text, "final text");
        assert!(loaded[0].card.is_some());

        update(&path, MODE_AGENT, "m1", None, Some(None)).unwrap();
        let loaded = load(&path, MODE_AGENT).unwrap();
        assert!(loaded[0].card.is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn clear_only_touches_its_mode_and_missing_update_errors() {
        let path = temp_db("clear");
        append(&path, Some("a1"), MODE_AGENT, "user", "a", None, 1).unwrap();
        append(&path, Some("c1"), MODE_CHAT, "user", "c", None, 2).unwrap();
        clear(&path, MODE_AGENT).unwrap();
        assert!(load(&path, MODE_AGENT).unwrap().is_empty());
        assert_eq!(load(&path, MODE_CHAT).unwrap().len(), 1);
        assert!(update(&path, MODE_AGENT, "a1", Some("x"), None).is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn invalid_mode_and_role_are_rejected() {
        let path = temp_db("invalid");
        assert!(append(&path, None, "hacker", "user", "x", None, 1).is_err());
        assert!(append(&path, None, MODE_CHAT, "root", "x", None, 1).is_err());
        assert!(load(&path, "nope").is_err());
        assert!(clear(&path, "nope").is_err());
        let _ = std::fs::remove_file(&path);
    }
}
