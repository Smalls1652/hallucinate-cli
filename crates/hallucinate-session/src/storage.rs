//! Session storage using JSONL format.
//!
//! Each session is stored as a JSONL file where each line represents
//! a message or event in the conversation history.

use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::PathBuf,
};

use chrono::{DateTime, Utc};
use hallucinate_agent::ai::types::Message;

/// A line in a session JSONL file.
#[derive(Debug, Clone)]
pub enum SessionEntry {
    /// A message in the conversation.
    Message { message: Box<Message> },

    /// An event (e.g., tool execution, state change).
    Event {
        timestamp: u64,
        event_type: String,
        data: serde_json::Value,
    },

    /// Session metadata, written when a session is created.
    Metadata {
        timestamp: u64,
        title: Option<String>,
        parent_session_id: Option<String>,
        cwd: String,
    },
}

// Custom serialization for SessionEntry to avoid duplicate "role" field issues
impl SessionEntry {
    /// Serialize the entry to JSON.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        match self {
            Self::Message { message } => {
                // Serialize the message directly (it has its own "role" tag)
                serde_json::to_string(message)
            },

            Self::Event {
                timestamp,
                event_type,
                data,
            } => serde_json::to_string(&serde_json::json!({
                "type": "event",
                "timestamp": timestamp,
                "event_type": event_type,
                "data": data,
            })),

            Self::Metadata {
                timestamp,
                title,
                parent_session_id,
                cwd,
            } => serde_json::to_string(&serde_json::json!({
                "type": "metadata",
                "timestamp": timestamp,
                "title": title,
                "parent_session_id": parent_session_id,
                "cwd": cwd,
            })),
        }
    }

    /// Deserialize from JSON.
    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        let value: serde_json::Value = serde_json::from_str(s)?;

        // Check for explicit type field first
        if let Some(type_str) = value.get("type").and_then(|t| t.as_str()) {
            match type_str {
                "event" => {
                    let timestamp = value.get("timestamp").and_then(|t| t.as_u64()).unwrap_or(0);

                    let event_type = value
                        .get("event_type")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string();

                    let data = value.get("data").cloned().unwrap_or(serde_json::json!({}));

                    return Ok(Self::Event {
                        timestamp,
                        event_type,
                        data,
                    });
                },

                "metadata" => {
                    return Ok(Self::Metadata {
                        timestamp: value.get("timestamp").and_then(|t| t.as_u64()).unwrap_or(0),
                        title: value
                            .get("title")
                            .and_then(|t| t.as_str())
                            .map(str::to_string),
                        parent_session_id: value
                            .get("parent_session_id")
                            .and_then(|t| t.as_str())
                            .map(str::to_string),
                        cwd: value
                            .get("cwd")
                            .and_then(|t| t.as_str())
                            .unwrap_or(".")
                            .to_string(),
                    });
                },

                _ => {},
            }
        }

        // No type field, try to deserialize as a Message
        // The Message enum uses "role" as its discriminant
        let message: Message = serde_json::from_value(value)?;

        Ok(Self::Message {
            message: Box::new(message),
        })
    }
}

impl SessionEntry {
    /// Create a message entry from a Message.
    pub fn from_message(message: Message) -> Self {
        Self::Message {
            message: Box::new(message),
        }
    }

    /// Create an event entry.
    pub fn from_event(
        event_type: impl Into<String>,
        data: serde_json::Value,
    ) -> Self {
        Self::Event {
            timestamp: Utc::now().timestamp_millis() as u64,
            event_type: event_type.into(),
            data,
        }
    }

    /// Create structured metadata for a session.
    pub fn from_metadata(
        title: Option<String>,
        parent_session_id: Option<String>,
        cwd: String,
    ) -> Self {
        Self::Metadata {
            timestamp: Utc::now().timestamp_millis() as u64,
            title,
            parent_session_id,
            cwd,
        }
    }
}

/// Metadata stored in an append-only JSONL session record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMetadata {
    /// The human-readable session title, if one has been assigned.
    pub title: Option<String>,

    /// The parent session ID when this session was forked.
    pub parent_session_id: Option<String>,

    /// The working directory used when the session was created or renamed.
    pub cwd: String,

    /// Unix timestamp in milliseconds when this metadata record was written.
    pub timestamp: u64,
}

/// A JSONL session file reader/writer.
pub struct SessionStorage {
    path: PathBuf,
}

impl SessionStorage {
    /// Create a new session storage for the given session ID.
    pub fn new(session_id: &str) -> Self {
        let path = hallucinate_agent::config::session_file(session_id)
            .unwrap_or_else(|| PathBuf::from(format!("/tmp/session-{}.jsonl", session_id)));

        Self { path }
    }

    /// Create a storage with an explicit path for isolated tests.
    #[cfg(test)]
    pub fn with_path(path: PathBuf) -> Self {
        Self { path }
    }

    /// Get the path to this session file.
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    /// Load all messages from the session file.
    pub fn load_messages(&self) -> Result<Vec<Message>, anyhow::Error> {
        Ok(self
            .load_all()?
            .into_iter()
            .filter_map(|entry| match entry {
                SessionEntry::Message { message } => Some(*message),
                _ => None,
            })
            .collect())
    }

    /// Load the newest durable compaction checkpoint, if the session has one.
    pub fn load_compaction_checkpoint(&self) -> Result<Option<(String, usize)>, anyhow::Error> {
        let mut checkpoint = None;

        for entry in self.load_all()? {
            let SessionEntry::Event {
                event_type, data, ..
            } = entry
            else {
                continue;
            };
            if event_type != "compaction_checkpoint" {
                continue;
            }

            let summary = data
                .get("summary")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("compaction checkpoint has no summary"))?;

            let archived_message_count = data
                .get("archived_message_count")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| {
                    anyhow::anyhow!("compaction checkpoint has no archived message count")
                })?
                .try_into()
                .map_err(|_| {
                    anyhow::anyhow!("compaction checkpoint archived message count is too large")
                })?;

            checkpoint = Some((summary.to_string(), archived_message_count));
        }

        Ok(checkpoint)
    }

    /// Load the most recently reported context token count, if present.
    ///
    /// Completed assistant responses report their provider usage through
    /// `token_usage` events. Compaction records the count immediately before
    /// its summary replaces part of the context, preserving the displayed
    /// count even when no later response has usage data.
    pub fn load_latest_context_token_count(&self) -> Result<Option<u64>, anyhow::Error> {
        let mut token_count = None;

        for entry in self.load_all()? {
            let SessionEntry::Event {
                event_type, data, ..
            } = entry
            else {
                continue;
            };

            let event_token_count = match event_type.as_str() {
                "token_usage" => data
                    .get("usage")
                    .and_then(|usage| usage.get("total_tokens"))
                    .and_then(serde_json::Value::as_u64),

                "compaction" => data
                    .get("context_token_count")
                    .and_then(serde_json::Value::as_u64),

                _ => None,
            };

            if let Some(event_token_count) = event_token_count {
                token_count = Some(event_token_count);
            }
        }

        Ok(token_count)
    }

    pub fn save_metadata(
        &self,
        title: Option<String>,
        parent_session_id: Option<String>,
        cwd: String,
    ) -> Result<(), anyhow::Error> {
        self.append_entry(SessionEntry::from_metadata(title, parent_session_id, cwd))
    }

    /// Load the most recent structured metadata entry, if this session has
    /// one. Metadata is append-only, so using the latest entry also supports
    /// updating a session name without rewriting its conversation history.
    pub fn load_metadata(&self) -> Result<Option<SessionMetadata>, anyhow::Error> {
        let mut metadata = None;

        for entry in self.load_all()? {
            if let SessionEntry::Metadata {
                timestamp,
                title,
                parent_session_id,
                cwd,
            } = entry
            {
                metadata = Some(SessionMetadata {
                    title,
                    parent_session_id,
                    cwd,
                    timestamp,
                });
            }
        }

        Ok(metadata)
    }

    /// Save a message to the session file.
    pub fn save_message(
        &self,
        message: &Message,
    ) -> Result<(), anyhow::Error> {
        self.append_entry(SessionEntry::from_message(message.clone()))
    }

    /// Save an event to the session file.
    pub fn save_event(
        &self,
        event_type: &str,
        data: serde_json::Value,
    ) -> Result<(), anyhow::Error> {
        self.append_entry(SessionEntry::from_event(event_type, data))
    }

    /// Append an entry to the session file.
    fn append_entry(
        &self,
        entry: SessionEntry,
    ) -> Result<(), anyhow::Error> {
        // Ensure parent directory exists
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;

        let line = entry.to_json()?;
        writeln!(file, "{}", line)?;

        Ok(())
    }

    /// Load all entries from the session file.
    ///
    /// Session files are append-only conversation logs. A malformed line is
    /// reported with its line number instead of being silently discarded;
    /// silently skipping it could make a resumed conversation use the wrong
    /// context or hide a partial write.
    pub fn load_all(&self) -> Result<Vec<SessionEntry>, anyhow::Error> {
        let mut entries = Vec::new();

        if !self.path.exists() {
            return Ok(entries);
        }

        let file = File::open(&self.path)?;
        let reader = BufReader::new(file);

        for (line_number, line) in reader.lines().enumerate() {
            let line_number = line_number + 1;
            let line = line.map_err(|error| {
                anyhow::anyhow!(
                    "failed reading session {} at line {}: {}",
                    hallucinate_agent::config::display_path(&self.path, "."),
                    line_number,
                    error
                )
            })?;

            if line.trim().is_empty() {
                continue;
            }

            let entry = SessionEntry::from_json(&line).map_err(|error| {
                anyhow::anyhow!(
                    "invalid session entry in {} at line {}: {}",
                    hallucinate_agent::config::display_path(&self.path, "."),
                    line_number,
                    error
                )
            })?;

            entries.push(entry);
        }

        Ok(entries)
    }

    /// Get the number of messages in the session.
    pub fn message_count(&self) -> Result<usize, anyhow::Error> {
        Ok(self.load_messages()?.len())
    }

    /// Check if the session file exists.
    pub fn exists(&self) -> bool {
        self.path.exists()
    }
}

/// List all session files in the sessions directory.
pub fn list_sessions() -> Result<Vec<SessionInfo>, anyhow::Error> {
    let mut sessions = Vec::new();

    if let Some(sessions_dir) = hallucinate_agent::config::sessions_dir() {
        if !sessions_dir.exists() {
            return Ok(sessions);
        }

        let entries = fs::read_dir(&sessions_dir)?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            if path.extension().map(|e| e == "jsonl").unwrap_or(false)
                && let Some(filename) = path.file_stem()
            {
                let session_id = filename.to_string_lossy().to_string();
                let metadata = match entry.metadata() {
                    Ok(metadata) => metadata,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error.into()),
                };
                let modified = metadata.modified().ok().map(DateTime::<Utc>::from);

                sessions.push(SessionInfo {
                    id: session_id,
                    path,
                    modified,
                });
            }
        }
    }

    // Sort by modification time, newest first
    sessions.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| b.id.cmp(&a.id)));

    Ok(sessions)
}

/// Information about a session.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    /// The session ID (filename without .jsonl extension).
    pub id: String,

    /// The full path to the session file.
    pub path: PathBuf,

    /// Last modification time.
    pub modified: Option<DateTime<Utc>>,
}

impl SessionInfo {
    /// Get a human-readable last modified string.
    pub fn modified_str(&self) -> String {
        self.modified
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "unknown".to_string())
    }
}

#[cfg(test)]
mod tests {
    use hallucinate_agent::ai::types::UserMessage;

    use super::*;

    #[test]
    fn test_session_storage_roundtrip() {
        let temp_dir = std::env::temp_dir().join("hallucinate-test-session");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let path = temp_dir.join("test.jsonl");
        let storage = SessionStorage::with_path(path.clone());

        // Save a message
        let msg = Message::User(UserMessage::new("Hello, world!"));
        storage.save_message(&msg).unwrap();

        // Load it back
        let loaded = storage.load_messages().unwrap();
        assert_eq!(loaded.len(), 1);

        match &loaded[0] {
            Message::User(u) => assert_eq!(u.content, "Hello, world!"),
            _ => panic!("Expected User message"),
        }

        // Clean up
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn structured_metadata_roundtrips() {
        let path = std::env::temp_dir().join(format!(
            "hallucinate-metadata-{}.jsonl",
            uuid::Uuid::new_v4()
        ));

        let storage = SessionStorage::with_path(path.clone());
        storage
            .save_metadata(
                Some("Session title".to_string()),
                Some("parent".to_string()),
                "/tmp".to_string(),
            )
            .unwrap();

        assert_eq!(
            storage.load_metadata().unwrap(),
            Some(SessionMetadata {
                title: Some("Session title".to_string()),
                parent_session_id: Some("parent".to_string()),
                cwd: "/tmp".to_string(),
                timestamp: storage
                    .load_all()
                    .unwrap()
                    .into_iter()
                    .find_map(|entry| match entry {
                        SessionEntry::Metadata { timestamp, .. } => Some(timestamp),
                        _ => None,
                    })
                    .unwrap(),
            })
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn loads_the_latest_compaction_checkpoint() {
        let path = std::env::temp_dir().join(format!(
            "hallucinate-checkpoint-{}.jsonl",
            uuid::Uuid::new_v4()
        ));

        let storage = SessionStorage::with_path(path.clone());
        storage
            .save_event(
                "compaction_checkpoint",
                serde_json::json!({ "summary": "first", "archived_message_count": 2 }),
            )
            .unwrap();

        storage
            .save_event(
                "compaction_checkpoint",
                serde_json::json!({ "summary": "updated", "archived_message_count": 5 }),
            )
            .unwrap();

        assert_eq!(
            storage.load_compaction_checkpoint().unwrap(),
            Some(("updated".to_string(), 5))
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn loads_the_latest_context_token_count_from_usage_and_compaction_events() {
        let path = std::env::temp_dir().join(format!(
            "hallucinate-token-usage-{}.jsonl",
            uuid::Uuid::new_v4()
        ));

        let storage = SessionStorage::with_path(path.clone());
        storage
            .save_event(
                "token_usage",
                serde_json::json!({ "usage": { "total_tokens": 10 } }),
            )
            .unwrap();
        storage
            .save_event(
                "compaction",
                serde_json::json!({ "context_token_count": 10 }),
            )
            .unwrap();
        storage
            .save_event(
                "token_usage",
                serde_json::json!({ "usage": { "total_tokens": 6 } }),
            )
            .unwrap();

        assert_eq!(storage.load_latest_context_token_count().unwrap(), Some(6));

        let _ = fs::remove_file(path);
    }
    #[test]
    fn malformed_entries_are_reported_with_their_line_number() {
        let path = std::env::temp_dir().join(format!(
            "hallucinate-corrupt-{}.jsonl",
            uuid::Uuid::new_v4()
        ));

        fs::write(
            &path,
            "{\"role\":\"user\",\"content\":\"ok\",\"timestamp\":1}\nnot json\n",
        )
        .unwrap();

        let error = SessionStorage::with_path(path.clone())
            .load_all()
            .unwrap_err();

        let message = error.to_string();
        assert!(message.contains("line 2"), "unexpected error: {message}");
        assert!(
            message.contains("invalid session entry"),
            "unexpected error: {message}"
        );

        let _ = fs::remove_file(path);
    }
}
