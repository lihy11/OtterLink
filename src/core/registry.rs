use std::{collections::HashMap, sync::Arc};

use anyhow::Result;
use serde::Serialize;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::core::persistence::Persistence;

#[derive(Clone)]
pub struct SessionRegistry {
    sessions: Arc<Mutex<HashMap<String, SessionInfo>>>,
    persistence: Persistence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub session_key: String,
    pub parent_session_id: Option<String>,
    pub runtime_session_ref: Option<String>,
    pub last_assistant_message: Option<String>,
}

impl SessionRegistry {
    pub async fn new(persistence: Persistence) -> Result<Self> {
        let sessions = persistence.load_sessions().await?;
        Ok(Self {
            sessions: Arc::new(Mutex::new(sessions)),
            persistence,
        })
    }

    pub async fn resolve(
        &self,
        session_key: &str,
        parent_session_key: Option<&str>,
    ) -> Result<SessionInfo> {
        let mut guard = self.sessions.lock().await;
        if let Some(existing) = guard.get(session_key) {
            return Ok(existing.clone());
        }

        let parent_session_id = parent_session_key.and_then(|key| guard.get(key).map(|s| s.session_id.clone()));
        let session = SessionInfo {
            session_id: format!("sess_{}", Uuid::new_v4().simple()),
            session_key: session_key.to_string(),
            parent_session_id,
            runtime_session_ref: None,
            last_assistant_message: None,
        };
        guard.insert(session_key.to_string(), session.clone());
        drop(guard);

        self.persistence.upsert_session(&session).await?;
        Ok(session)
    }

    pub async fn get_by_session_id(&self, session_id: &str) -> Option<SessionInfo> {
        let guard = self.sessions.lock().await;
        guard.values().find(|s| s.session_id == session_id).cloned()
    }

    pub async fn get_by_session_key(&self, session_key: &str) -> Option<SessionInfo> {
        let guard = self.sessions.lock().await;
        guard.get(session_key).cloned()
    }

    pub async fn update_runtime_state(
        &self,
        session_id: &str,
        runtime_session_ref: Option<String>,
        last_assistant_message: Option<String>,
    ) -> Result<()> {
        let mut guard = self.sessions.lock().await;
        let mut session_key: Option<String> = None;
        if let Some(session) = guard.values_mut().find(|s| s.session_id == session_id) {
            if let Some(runtime_ref) = runtime_session_ref {
                session.runtime_session_ref = Some(runtime_ref);
            }
            if let Some(message) = last_assistant_message {
                session.last_assistant_message = Some(message);
            }
            session_key = Some(session.session_key.clone());
        }
        drop(guard);

        if let Some(key) = session_key {
            if let Some(session) = self.get_by_session_key(&key).await {
                self.persistence
                    .update_session_runtime_state(
                        &key,
                        session.runtime_session_ref.as_deref(),
                        session.last_assistant_message.as_deref(),
                    )
                    .await?;
            }
        }
        Ok(())
    }

    pub async fn replace_runtime_state(
        &self,
        session_key: &str,
        runtime_session_ref: Option<String>,
        last_assistant_message: Option<String>,
    ) -> Result<()> {
        let mut guard = self.sessions.lock().await;
        if let Some(session) = guard.get_mut(session_key) {
            session.runtime_session_ref = runtime_session_ref.clone();
            session.last_assistant_message = last_assistant_message.clone();
        }
        drop(guard);

        self.persistence
            .update_session_runtime_state(
                session_key,
                runtime_session_ref.as_deref(),
                last_assistant_message.as_deref(),
            )
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::core::persistence::Persistence;

    #[tokio::test]
    async fn resolves_parent_session_when_it_exists() {
        let db = PathBuf::from(format!("/tmp/otterlink-registry-{}.db", uuid::Uuid::new_v4()));
        let persistence = Persistence::new(db);
        persistence.init().await.unwrap();
        let registry = SessionRegistry::new(persistence).await.unwrap();

        let parent = registry.resolve("feishu:group:oc_root", None).await.unwrap();
        let child = registry
            .resolve("feishu:thread:oc_root:th_1", Some("feishu:group:oc_root"))
            .await
            .unwrap();

        assert_eq!(child.parent_session_id, Some(parent.session_id));
    }
}
