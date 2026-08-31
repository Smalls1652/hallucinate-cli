//! Session manager.
//!
//! Provides session CRUD operations, creation with unique IDs, and
//! integration with the agent loop.

use std::fs;

use chrono::Utc;
use hallucinate_agent::{ai::types::Message, config::paths};
use uuid::Uuid;

use crate::session::{
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
        let session_id = Uuid::new_v4().to_string();

        // Ensure sessions directory exists
        if let Some(sessions_dir) = paths::sessions_dir() {
            fs::create_dir_all(&sessions_dir)?;
        }

        // Determine working directory
        let cwd = options
            .cwd
            .or_else(|| SessionCwd::load(&session_id).ok())
            .unwrap_or_else(|| {
                std::env::current_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| ".".to_string())
            });

        // Save the working directory sidecar as a compatibility fallback for
        // sessions created by older versions.
        SessionCwd::save(&session_id, &cwd)?;

        // Create initial session file
        let storage = SessionStorage::new(&session_id);

        let title = options.title.or_else(|| Some("New Session".to_string()));

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

        let parent_cwd = parent_storage
            .load_metadata()?
            .map(|(_, _, cwd)| cwd)
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

        let (title, parent_session_id, saved_cwd) = metadata.unwrap_or((
            None,
            None,
            SessionCwd::load(session_id).unwrap_or_else(|_| ".".to_string()),
        ));

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
        crate::session::storage::list_sessions()
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
