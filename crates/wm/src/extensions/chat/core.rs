//! Chat domain, validation and wire models; independent of transports and UI.
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
/// Message cap per sidecar. Older messages are dropped first.
pub const MAX_MESSAGES: usize = 500;
pub const MAX_AUTHOR_CHARS: usize = 32;
pub const MAX_TEXT_CHARS: usize = 2000;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: u64,
    pub author: String,
    pub text: String,
    pub at: DateTime<Utc>,
}

#[derive(Debug)]
pub struct Room {
    title: String,
    node: Option<String>,
    /// Identifies this process instance. A browser that sees a new epoch
    /// clears its local view instead of waiting for ids that restarted at 1.
    epoch: String,
    messages: VecDeque<ChatMessage>,
    next_id: u64,
}

impl Room {
    pub fn new(title: String, node: Option<String>) -> Self {
        Self {
            title,
            node,
            epoch: uuid::Uuid::new_v4().to_string(),
            messages: VecDeque::new(),
            next_id: 1,
        }
    }

    fn push(&mut self, author: String, text: String) -> ChatMessage {
        let message = ChatMessage {
            id: self.next_id,
            author,
            text,
            at: Utc::now(),
        };
        self.next_id += 1;
        self.messages.push_back(message.clone());
        while self.messages.len() > MAX_MESSAGES {
            self.messages.pop_front();
        }
        message
    }

    fn after(&self, after: u64) -> Vec<ChatMessage> {
        self.messages
            .iter()
            .filter(|message| message.id > after)
            .cloned()
            .collect()
    }
}

#[derive(Clone)]
pub struct ChatState {
    room: Arc<Mutex<Room>>,
}

impl ChatState {
    pub fn new(title: String, node: Option<String>) -> Self {
        Self {
            room: Arc::new(Mutex::new(Room::new(title, node))),
        }
    }

    pub fn room(&self) -> MutexGuard<'_, Room> {
        self.room
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub after: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PostInput {
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InfoResponse {
    pub title: String,
    pub node: Option<String>,
    pub epoch: String,
    pub messages: usize,
    pub max_messages: usize,
    pub max_text: usize,
    pub subtitle: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MessagesResponse {
    pub epoch: String,
    pub cursor: u64,
    pub messages: Vec<ChatMessage>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PostResponse {
    pub epoch: String,
    pub message: ChatMessage,
}

pub fn clean_author(raw: &str) -> Result<String, String> {
    let author = raw.trim();
    let author = if author.is_empty() {
        "anônimo"
    } else {
        author
    };
    if author.chars().count() > MAX_AUTHOR_CHARS {
        return Err(format!(
            "o nome deve ter no máximo {MAX_AUTHOR_CHARS} caracteres"
        ));
    }
    if author.chars().any(char::is_control) {
        return Err("o nome contém caracteres de controle".into());
    }
    Ok(author.to_string())
}

pub fn clean_text(raw: &str) -> Result<String, String> {
    let text = raw.trim();
    if text.is_empty() {
        return Err("a mensagem não pode estar vazia".into());
    }
    if text.chars().count() > MAX_TEXT_CHARS {
        return Err(format!(
            "a mensagem deve ter no máximo {MAX_TEXT_CHARS} caracteres"
        ));
    }
    if text
        .chars()
        .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\t'))
    {
        return Err("a mensagem contém caracteres de controle".into());
    }
    Ok(text.to_string())
}

impl Room {
    pub fn info(&self) -> InfoResponse {
    let room = self;
    let subtitle = match &room.node {
        Some(node) => format!(
            "Sala aberta no nó {node}. Qualquer pessoa com acesso a esta porta pode conversar."
        ),
        None => "Sala aberta: qualquer pessoa com acesso a esta porta pode conversar.".into(),
    };
    InfoResponse {
        title: room.title.clone(),
        node: room.node.clone(),
        epoch: room.epoch.clone(),
        messages: room.messages.len(),
        max_messages: MAX_MESSAGES,
        max_text: MAX_TEXT_CHARS,
        subtitle,
    }
    }
    pub fn list(&self, after: u64) -> MessagesResponse {
    let room = self;
    let messages = room.after(after);
    let cursor = messages
        .last()
        .map(|message| message.id)
        .unwrap_or(after);
    MessagesResponse {
        epoch: room.epoch.clone(),
        cursor,
        messages,
    }
    }
    pub fn send(&mut self, input: PostInput) -> Result<PostResponse, String> {
        let author = clean_author(input.author.as_deref().unwrap_or(""))?;
        let text = clean_text(input.text.as_deref().unwrap_or(""))?;
        let message = self.push(author, text);
        Ok(PostResponse { epoch: self.epoch.clone(), message })
    }
}

#[cfg(test)]
mod tests {
use super::*;
    #[test]
    fn author_defaults_and_rejects_controls() {
        assert_eq!(clean_author("  ").unwrap(), "anônimo");
        assert_eq!(clean_author(" Ana ").unwrap(), "Ana");
        assert!(clean_author("a\u{0}b").is_err());
        assert!(clean_author(&"x".repeat(MAX_AUTHOR_CHARS + 1)).is_err());
        assert!(clean_author(&"x".repeat(MAX_AUTHOR_CHARS)).is_ok());
    }

    #[test]
    fn text_is_trimmed_capped_and_keeps_newlines() {
        assert_eq!(clean_text("  oi\nmundo  ").unwrap(), "oi\nmundo");
        assert!(clean_text("   ").is_err());
        assert!(clean_text("a\u{7}b").is_err());
        assert!(clean_text(&"x".repeat(MAX_TEXT_CHARS + 1)).is_err());
        assert!(clean_text(&"x".repeat(MAX_TEXT_CHARS)).is_ok());
    }

    #[test]
    fn unicode_limits_count_scalars_not_utf8_bytes() {
        assert!(clean_author(&"🦀".repeat(MAX_AUTHOR_CHARS)).is_ok());
        assert!(clean_author(&"🦀".repeat(MAX_AUTHOR_CHARS + 1)).is_err());
        assert!(clean_text(&"á".repeat(MAX_TEXT_CHARS)).is_ok());
        assert!(clean_text(&"á".repeat(MAX_TEXT_CHARS + 1)).is_err());
        assert_eq!(clean_text(" olá\t世界\n🦀 ").unwrap(), "olá\t世界\n🦀");
        assert!(clean_author("a\tb").is_err());
        assert!(clean_text("a\u{0085}b").is_err());
    }

    #[test]
    fn domain_send_validates_before_mutating_room() {
        let mut room = Room::new("Sala".into(), None);
        assert!(room.send(PostInput { author: None, text: Some("  ".into()) }).is_err());
        assert_eq!(room.info().messages, 0);
        let posted = room.send(PostInput { author: None, text: Some(" oi ".into()) }).unwrap();
        assert_eq!(posted.message.id, 1);
        assert_eq!(posted.message.author, "anônimo");
        assert_eq!(posted.message.text, "oi");
        assert_eq!(room.list(0).epoch, posted.epoch);
        assert_eq!(room.list(0).cursor, 1);
        assert!(room.list(1).messages.is_empty());
    }

    #[test]
    fn room_filters_after_and_caps_history() {
        let mut room = Room::new("Sala".into(), None);
        for index in 0..(MAX_MESSAGES + 2) {
            room.push("Ana".into(), format!("mensagem {index}"));
        }
        assert_eq!(room.messages.len(), MAX_MESSAGES);
        let after_last = room.after(room.next_id - 1);
        assert!(after_last.is_empty());
        let all = room.after(0);
        assert_eq!(all.first().unwrap().id, 3);
        assert_eq!(all.last().unwrap().id, MAX_MESSAGES as u64 + 2);
    }
}
