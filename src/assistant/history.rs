//! The questions asked and Claude's answers, kept in `chat-history.json` in
//! the folder the app runs from, so they survive restarts.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::assistant::Suggestion;

pub const HISTORY_FILE: &str = "chat-history.json";
/// Older exchanges are dropped when saving, so the file can't grow forever.
pub const MAX_KEPT: usize = 200;

/// One question and what came back: a suggested query, or why asking failed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Exchange {
    pub question: String,
    /// Names of the files attached to the question.
    #[serde(default)]
    pub attachments: Vec<String>,
    pub answer: Result<Suggestion, String>,
}

pub fn path() -> PathBuf {
    PathBuf::from(HISTORY_FILE)
}

/// Reads the history. A missing file is an empty history; an unreadable one
/// is moved aside to `<file>.bad`, so it isn't overwritten, and the history
/// starts empty.
pub fn load(path: &Path) -> Vec<Exchange> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            tracing::warn!(error = %e, file = %path.display(), "could not read the chat history");
            return Vec::new();
        }
    };
    match serde_json::from_str(&text) {
        Ok(chat) => chat,
        Err(e) => {
            let aside = path.with_extension("json.bad");
            tracing::warn!(error = %e, moved_to = %aside.display(), "the chat history is unreadable; starting a new one");
            let _ = std::fs::rename(path, aside);
            Vec::new()
        }
    }
}

/// Saves the newest `MAX_KEPT` exchanges. Writes a temporary file first, so
/// a crash can't leave half a file.
pub fn save(path: &Path, chat: &[Exchange]) -> Result<(), Error> {
    let kept = &chat[chat.len().saturating_sub(MAX_KEPT)..];
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_string_pretty(kept)?)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}
