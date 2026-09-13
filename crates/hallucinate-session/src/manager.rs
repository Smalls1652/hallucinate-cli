//! Session manager.
//!
//! Provides session CRUD operations, creation with unique IDs, and
//! integration with the agent loop.

use std::fs;

use chrono::Utc;
use hallucinate_agent::{agent::types::AgentEvent, ai::types::Message, config::paths};
use uuid::Uuid;

use crate::{
    cwd::SessionCwd,
    storage::{SessionInfo, SessionStorage},
};

/// Options for creating a new session.
#[derive(Debug, Clone)]
pub struct SessionCreateOptions {
    /// Optional title for the session.
    pub title: Option<String>,

    /// Optional parent session ID (for forking).
    pub parent_session_id: Option<String>,

    /// Working directory for the session.
    pub cwd: Option<String>,
}

/// A session with its metadata.
#[derive(Debug, Clone)]
pub struct Session {
    /// The session ID.
    pub id: String,

    /// Optional title.
    pub title: Option<String>,

    /// Parent session ID (if forked).
    pub parent_session_id: Option<String>,

    /// Working directory.
    pub cwd: String,

    /// Number of messages.
    pub message_count: usize,

    /// Last modified timestamp.
    pub modified: u64,
}

/// Session manager for creating, listing, and switching sessions.
pub struct SessionManager {
    /// The current session.
    current_session: Option<Session>,
}

impl SessionManager {
    /// Create a new session manager.
    pub fn new() -> Self {
        Self {
            current_session: None,
        }
    }

    /// Create a new session.
    pub fn create(
        &mut self,
        options: SessionCreateOptions,
    ) -> Result<Session, anyhow::Error> {
        self.discard_empty_current()?;

        let session_id = Uuid::new_v4().to_string();

        // Ensure sessions directory exists
        if let Some(sessions_dir) = paths::sessions_dir() {
            fs::create_dir_all(&sessions_dir)?;
        }

        // Determine working directory
        let cwd = options.cwd.unwrap_or_else(|| {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| ".".to_string())
        });

        // Create initial session file
        let storage = SessionStorage::new(&session_id);

        let title = options.title;

        storage.save_metadata(
            title.clone(),
            options.parent_session_id.clone(),
            cwd.clone(),
        )?;

        let session = Session {
            id: session_id.clone(),
            title,
            parent_session_id: options.parent_session_id,
            cwd,
            message_count: 0,
            modified: Utc::now().timestamp_millis() as u64,
        };

        self.current_session = Some(session.clone());

        Ok(session)
    }

    /// Fork an existing session (create a child session with the same history).
    pub fn fork(
        &mut self,
        parent_id: &str,
    ) -> Result<Session, anyhow::Error> {
        validate_session_id(parent_id)?;
        let parent_storage = SessionStorage::new(parent_id);

        let messages = parent_storage.load_messages()?;
        let checkpoint = parent_storage.load_compaction_checkpoint()?;

        let parent_cwd = parent_storage
            .load_metadata()?
            .map(|metadata| metadata.cwd)
            .or_else(|| SessionCwd::load(parent_id).ok());

        // Create a new session
        let mut session = self.create(SessionCreateOptions {
            title: Some(format!(
                "Fork of session {}",
                &parent_id[..8.min(parent_id.len())]
            )),
            parent_session_id: Some(parent_id.to_string()),
            cwd: parent_cwd,
        })?;

        // Copy messages to the new session
        let new_storage = SessionStorage::new(&session.id);

        for msg in &messages {
            new_storage.save_message(msg)?;
        }
        if let Some((summary, archived_message_count)) = checkpoint {
            new_storage.save_event(
                "compaction_checkpoint",
                serde_json::json!({
                    "summary": summary,
                    "archived_message_count": archived_message_count,
                }),
            )?;
        }

        // Update message count and modification time after copying history.
        session.message_count = messages.len();
        session.modified = Utc::now().timestamp_millis() as u64;

        self.current_session = Some(session.clone());

        Ok(session)
    }

    /// Resume an existing session.
    pub fn resume(
        &mut self,
        session_id: &str,
    ) -> Result<Session, anyhow::Error> {
        validate_session_id(session_id)?;
        let storage = SessionStorage::new(session_id);

        if !storage.exists() {
            return Err(anyhow::anyhow!("Session '{}' not found", session_id));
        }

        let metadata = storage.load_metadata()?;

        let metadata = metadata.unwrap_or_else(|| crate::storage::SessionMetadata {
            title: None,
            parent_session_id: None,
            cwd: SessionCwd::load(session_id).unwrap_or_else(|_| ".".to_string()),
            timestamp: 0,
        });

        let title = metadata.title;

        let parent_session_id = metadata.parent_session_id;

        let saved_cwd = metadata.cwd;

        let cwd = if std::path::Path::new(&saved_cwd).is_dir() {
            saved_cwd
        } else {
            std::env::current_dir()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_else(|_| ".".to_string())
        };

        let message_count = storage.message_count()?;

        let session = Session {
            id: session_id.to_string(),
            title,
            parent_session_id,
            cwd,
            message_count,
            modified: Utc::now().timestamp_millis() as u64,
        };

        if self.current_id() != Some(session_id) {
            self.discard_empty_current()?;
        }
        self.current_session = Some(session.clone());

        Ok(session)
    }

    /// Get the current session.
    pub fn current(&self) -> Option<&Session> {
        self.current_session.as_ref()
    }

    /// Get the current session ID.
    pub fn current_id(&self) -> Option<&str> {
        self.current_session.as_ref().map(|s| s.id.as_str())
    }

    /// List all sessions.
    pub fn list() -> Result<Vec<SessionInfo>, anyhow::Error> {
        SessionInfo::list_all()
    }

    /// Remove persisted sessions that contain no conversation messages.
    ///
    /// Metadata and lifecycle events alone do not make a session resumable as a
    /// conversation. Malformed or unreadable session files are reported rather
    /// than being treated as empty.
    pub fn discard_historical_empty_sessions() -> Result<usize, anyhow::Error> {
        let mut discarded_count = 0;

        for session in Self::list()? {
            let storage = SessionStorage::new(&session.id);
            if storage.message_count()? == 0 {
                Self::delete_session(&session.id)?;
                discarded_count += 1;
            }
        }

        Ok(discarded_count)
    }

    /// Get the storage for the current session.
    pub fn current_storage(&self) -> Option<SessionStorage> {
        self.current_session
            .as_ref()
            .map(|s| SessionStorage::new(&s.id))
    }

    /// Save a message to the current session.
    pub fn save_message(
        &mut self,
        message: &Message,
    ) -> Result<(), anyhow::Error> {
        if let Some(storage) = self.current_storage() {
            storage.save_message(message)?;

            if let Some(session) = self.current_session.as_mut() {
                session.message_count += 1;
                session.modified = Utc::now().timestamp_millis() as u64;
            }
        }

        Ok(())
    }

    /// Persist an agent lifecycle event using the stable session event schema.
    ///
    /// Streaming deltas and completed assistant messages return successfully
    /// without writing an event because completed messages are archived through
    /// [`Self::save_message`].
    pub fn save_agent_event(
        &mut self,
        event: &AgentEvent,
    ) -> Result<(), anyhow::Error> {
        if let Some((event_type, data)) = crate::events::archive_event(event) {
            self.save_event(event_type, data)?;
        }

        Ok(())
    }

    /// Persist a compaction checkpoint for session-resume reconstruction.
    pub fn save_compaction_checkpoint(
        &mut self,
        summary: &str,
        archived_message_count: usize,
    ) -> Result<(), anyhow::Error> {
        self.save_event(
            "compaction_checkpoint",
            serde_json::json!({
                "summary": summary,
                "archived_message_count": archived_message_count,
            }),
        )
    }

    /// Save an event to the current session.
    pub fn save_event(
        &mut self,
        event_type: &str,
        data: serde_json::Value,
    ) -> Result<(), anyhow::Error> {
        if let Some(storage) = self.current_storage() {
            storage.save_event(event_type, data)?;
            self.touch();
        }

        Ok(())
    }

    /// Change the display name of the current session.
    ///
    /// Session files are append-only. A metadata entry is appended rather
    /// than rewriting the conversation, and `load_metadata` resolves the
    /// latest entry when the session is resumed.
    pub fn rename_current(
        &mut self,
        title: Option<String>,
    ) -> Result<(), anyhow::Error> {
        let session = self
            .current_session
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("there is no current session"))?;

        let title = title.filter(|title| !title.trim().is_empty());

        let storage = SessionStorage::new(&session.id);
        storage.save_metadata(
            title.clone(),
            session.parent_session_id.clone(),
            session.cwd.clone(),
        )?;
        session.title = title;
        session.modified = Utc::now().timestamp_millis() as u64;

        Ok(())
    }

    /// Delete the current session when it has no persisted conversation
    /// messages.
    ///
    /// Session metadata and lifecycle events do not make a session non-empty:
    /// only user, assistant, or tool messages represent a conversation worth
    /// retaining. The current session is cleared whether or not it was empty.
    pub fn discard_empty_current(&mut self) -> Result<bool, anyhow::Error> {
        let Some(session) = self.current_session.take() else {
            return Ok(false);
        };

        if session.message_count != 0 {
            self.current_session = Some(session);
            return Ok(false);
        }

        Self::delete_session(&session.id)?;

        Ok(true)
    }

    /// Delete a session.
    pub fn delete_session(session_id: &str) -> Result<(), anyhow::Error> {
        validate_session_id(session_id)?;
        let storage = SessionStorage::new(session_id);
        if storage.exists() {
            fs::remove_file(storage.path())?;
        }

        // Also delete the cwd file
        SessionCwd::delete(session_id)?;

        Ok(())
    }

    /// Update the current session's modified timestamp.
    pub fn touch(&mut self) {
        if let Some(session) = self.current_session.as_mut() {
            session.modified = Utc::now().timestamp_millis() as u64;
        }
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

fn validate_session_id(session_id: &str) -> Result<(), anyhow::Error> {
    if session_id.is_empty()
        || !session_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        return Err(anyhow::anyhow!("invalid session id"));
    }

    Ok(())
}

impl SessionInfo {
    /// List all sessions (convenience method).
    pub fn list_all() -> Result<Vec<SessionInfo>, anyhow::Error> {
        crate::storage::list_sessions()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_creation() {
        let mut manager = SessionManager::new();

        let session = manager
            .create(SessionCreateOptions {
                title: Some("Test Session".to_string()),
                parent_session_id: None,
                cwd: None,
            })
            .unwrap();

        assert!(!session.id.is_empty());
        assert_eq!(session.title, Some("Test Session".to_string()));
        assert_eq!(manager.current().unwrap().id, session.id);

        SessionManager::delete_session(&session.id).unwrap();
    }

    #[test]
    fn discarding_historical_empty_sessions_removes_only_empty_sessions() {
        let empty_session_id = format!("empty-history-{}", Uuid::new_v4());
        let populated_session_id = format!("populated-history-{}", Uuid::new_v4());
        let cwd = std::env::temp_dir().to_string_lossy().into_owned();

        SessionStorage::new(&empty_session_id)
            .save_metadata(None, None, cwd.clone())
            .unwrap();
        SessionStorage::new(&populated_session_id)
            .save_metadata(None, None, cwd)
            .unwrap();
        SessionStorage::new(&populated_session_id)
            .save_message(&Message::User(
                hallucinate_agent::ai::types::UserMessage::new("keep me"),
            ))
            .unwrap();

        let discarded_count = SessionManager::discard_historical_empty_sessions().unwrap();

        assert!(discarded_count >= 1);
        assert!(!SessionStorage::new(&empty_session_id).exists());
        assert!(SessionStorage::new(&populated_session_id).exists());

        SessionManager::delete_session(&populated_session_id).unwrap();
    }

    #[test]
    fn creating_a_session_discards_the_previous_empty_session() {
        let mut manager = SessionManager::new();
        let cwd = std::env::temp_dir().to_string_lossy().into_owned();

        let empty_session = manager
            .create(SessionCreateOptions {
                title: None,
                parent_session_id: None,
                cwd: Some(cwd.clone()),
            })
            .unwrap();
        let next_session = manager
            .create(SessionCreateOptions {
                title: None,
                parent_session_id: None,
                cwd: Some(cwd),
            })
            .unwrap();

        assert!(!SessionStorage::new(&empty_session.id).exists());
        assert!(SessionStorage::new(&next_session.id).exists());

        SessionManager::delete_session(&next_session.id).unwrap();
    }

    #[test]
    fn test_session_list_empty() {
        // Just verify it returns without error.
        let sessions = SessionManager::list();

        assert!(sessions.is_ok(), "Session listing should succeed");
    }

    #[test]
    fn rejects_path_traversal_session_ids() {
        let mut manager = SessionManager::new();

        assert!(manager.resume("../outside").is_err());
        assert!(SessionManager::delete_session("../outside").is_err());
    }

    #[test]
    fn fork_copies_history_and_parent_metadata_for_short_ids() {
        let parent_id = format!("fork-test-{}", Uuid::new_v4());

        let parent_storage = SessionStorage::new(&parent_id);
        parent_storage
            .save_metadata(None, None, "/tmp".to_string())
            .unwrap();
        parent_storage
            .save_message(&Message::User(
                hallucinate_agent::ai::types::UserMessage::new("hello"),
            ))
            .unwrap();

        let mut manager = SessionManager::new();
        let child = manager.fork(&parent_id).unwrap();

        assert_eq!(child.parent_session_id.as_deref(), Some(parent_id.as_str()));
        assert_eq!(child.cwd, "/tmp");
        assert_eq!(child.message_count, 1);
        assert_eq!(
            SessionStorage::new(&child.id)
                .load_messages()
                .unwrap()
                .len(),
            1
        );

        SessionManager::delete_session(&parent_id).unwrap();
        SessionManager::delete_session(&child.id).unwrap();
    }

    #[test]
    fn resume_prefers_jsonl_metadata_over_a_legacy_cwd_sidecar() {
        let session_id = format!("metadata-wins-{}", Uuid::new_v4());
        let jsonl_cwd = std::env::temp_dir().to_string_lossy().into_owned();
        let sidecar_cwd = "/path/that/does/not/exist";

        let storage = SessionStorage::new(&session_id);
        storage
            .save_metadata(Some("JSONL".to_string()), None, jsonl_cwd.clone())
            .unwrap();

        let sidecar = storage.path().with_extension("cwd.json");
        fs::write(
            &sidecar,
            serde_json::json!({ "cwd": sidecar_cwd, "timestamp": 1 }).to_string(),
        )
        .unwrap();

        let mut manager = SessionManager::new();
        let session = manager.resume(&session_id).unwrap();
        assert_eq!(session.title.as_deref(), Some("JSONL"));
        assert_eq!(session.cwd, jsonl_cwd);

        SessionManager::delete_session(&session_id).unwrap();
    }

    #[test]
    fn resume_uses_a_legacy_cwd_sidecar_when_jsonl_metadata_is_absent() {
        let session_id = format!("legacy-cwd-{}", Uuid::new_v4());
        let storage = SessionStorage::new(&session_id);
        storage
            .save_message(&Message::User(
                hallucinate_agent::ai::types::UserMessage::new("legacy"),
            ))
            .unwrap();

        let cwd = std::env::temp_dir().to_string_lossy().into_owned();
        fs::write(
            storage.path().with_extension("cwd.json"),
            serde_json::json!({ "cwd": cwd, "timestamp": 1 }).to_string(),
        )
        .unwrap();

        let mut manager = SessionManager::new();
        assert_eq!(manager.resume(&session_id).unwrap().cwd, cwd);

        SessionManager::delete_session(&session_id).unwrap();
    }

    #[test]
    fn new_sessions_do_not_create_legacy_cwd_sidecars() {
        let mut manager = SessionManager::new();
        let session = manager
            .create(SessionCreateOptions {
                title: None,
                parent_session_id: None,
                cwd: Some(std::env::temp_dir().to_string_lossy().into_owned()),
            })
            .unwrap();

        assert!(SessionStorage::new(&session.id).exists());
        assert!(
            !SessionStorage::new(&session.id)
                .path()
                .with_extension("cwd.json")
                .exists()
        );

        SessionManager::delete_session(&session.id).unwrap();
    }

    #[test]
    fn resume_falls_back_when_saved_cwd_is_missing() {
        let mut manager = SessionManager::new();

        let session = manager
            .create(SessionCreateOptions {
                title: None,
                parent_session_id: None,
                cwd: Some("/path/that/does/not/exist".to_string()),
            })
            .unwrap();

        let expected = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .into_owned();

        let mut resumed_manager = SessionManager::new();
        assert_eq!(resumed_manager.resume(&session.id).unwrap().cwd, expected);

        SessionManager::delete_session(&session.id).unwrap();
    }

    #[test]
    fn renaming_appends_metadata_and_survives_resume() {
        let mut manager = SessionManager::new();

        let session = manager
            .create(SessionCreateOptions {
                title: Some("Original".to_string()),
                parent_session_id: None,
                cwd: Some(std::env::temp_dir().to_string_lossy().into_owned()),
            })
            .unwrap();

        manager
            .rename_current(Some("Renamed session".to_string()))
            .unwrap();

        assert_eq!(
            manager.current().unwrap().title.as_deref(),
            Some("Renamed session")
        );

        let mut resumed_manager = SessionManager::new();

        let resumed = resumed_manager.resume(&session.id).unwrap();
        assert_eq!(resumed.title.as_deref(), Some("Renamed session"));

        SessionManager::delete_session(&session.id).unwrap();
    }

    #[test]
    fn session_messages_and_metadata_survive_resume_and_delete() {
        let cwd = std::env::temp_dir();

        let mut manager = SessionManager::new();

        let session = manager
            .create(SessionCreateOptions {
                title: Some("Persistent session".to_string()),
                parent_session_id: None,
                cwd: Some(cwd.to_string_lossy().into_owned()),
            })
            .unwrap();

        manager
            .save_message(&Message::User(
                hallucinate_agent::ai::types::UserMessage::new("hello"),
            ))
            .unwrap();
        manager
            .save_event("test_event", serde_json::json!({ "ok": true }))
            .unwrap();

        let mut resumed_manager = SessionManager::new();

        let resumed = resumed_manager.resume(&session.id).unwrap();
        assert_eq!(resumed.title.as_deref(), Some("Persistent session"));
        assert_eq!(resumed.cwd, cwd.to_string_lossy());
        assert_eq!(resumed.message_count, 1);
        assert_eq!(
            resumed_manager
                .current_storage()
                .unwrap()
                .load_messages()
                .unwrap()
                .len(),
            1
        );

        SessionManager::delete_session(&session.id).unwrap();
        assert!(
            !hallucinate_agent::config::paths::session_file(&session.id)
                .unwrap()
                .exists()
        );
        assert!(
            !hallucinate_agent::config::paths::session_file(&session.id)
                .unwrap()
                .with_extension("cwd.json")
                .exists()
        );
    }
}
