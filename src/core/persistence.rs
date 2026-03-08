use std::{collections::HashMap, path::PathBuf};

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use uuid::Uuid;

use crate::core::{registry::SessionInfo, support::now_unix};

#[derive(Clone)]
pub struct Persistence {
    db_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnRecord {
    pub turn_id: String,
    pub session_id: String,
    pub input_text: String,
    pub status: String,
    pub final_text: Option<String>,
    pub error_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeInstance {
    pub runtime_id: String,
    pub session_key: String,
    pub label: String,
    pub agent_kind: String,
    pub workspace_path: String,
    pub runtime_session_ref: Option<String>,
    pub tag: Option<String>,
    pub prompt_preview: Option<String>,
    pub last_assistant_message: Option<String>,
    pub is_active: bool,
}

impl Persistence {
    pub fn new(db_path: PathBuf) -> Self {
        Self { db_path }
    }

    pub async fn init(&self) -> Result<()> {
        let path = self.db_path.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("create db dir failed: {:?}", parent))?;
            }

            let conn = Connection::open(&path)
                .with_context(|| format!("open sqlite failed: {:?}", path))?;
            conn.execute_batch(
                r#"
                PRAGMA journal_mode = WAL;
                PRAGMA synchronous = NORMAL;

                CREATE TABLE IF NOT EXISTS core_sessions (
                    session_key TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL UNIQUE,
                    parent_session_id TEXT,
                    runtime_session_ref TEXT,
                    last_assistant_message TEXT,
                    updated_at INTEGER NOT NULL
                );

                CREATE TABLE IF NOT EXISTS core_turns (
                    turn_id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    input_text TEXT NOT NULL,
                    status TEXT NOT NULL,
                    final_text TEXT,
                    error_text TEXT,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );

                CREATE TABLE IF NOT EXISTS runtime_instances (
                    runtime_id TEXT PRIMARY KEY,
                    session_key TEXT NOT NULL,
                    label TEXT NOT NULL,
                    agent_kind TEXT NOT NULL,
                    workspace_path TEXT NOT NULL,
                    runtime_session_ref TEXT,
                    tag TEXT,
                    prompt_preview TEXT,
                    last_assistant_message TEXT,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );

                CREATE TABLE IF NOT EXISTS conversation_bindings (
                    session_key TEXT PRIMARY KEY,
                    active_runtime_id TEXT NOT NULL,
                    updated_at INTEGER NOT NULL
                );
                "#,
            )
            .context("init sqlite schema failed")?;
            let _ = conn.execute("ALTER TABLE runtime_instances ADD COLUMN tag TEXT", []);
            let _ = conn.execute("ALTER TABLE runtime_instances ADD COLUMN prompt_preview TEXT", []);
            Ok(())
        })
        .await
        .context("join sqlite init task failed")??;
        Ok(())
    }

    pub async fn load_sessions(&self) -> Result<HashMap<String, SessionInfo>> {
        let path = self.db_path.clone();
        let rows = tokio::task::spawn_blocking(move || -> Result<Vec<SessionInfo>> {
            let conn = Connection::open(&path)
                .with_context(|| format!("open sqlite failed: {:?}", path))?;
            let mut stmt = conn.prepare(
                "SELECT session_key, session_id, parent_session_id, runtime_session_ref, last_assistant_message FROM core_sessions",
            )?;
            let mapped = stmt.query_map([], |row| {
                Ok(SessionInfo {
                    session_key: row.get(0)?,
                    session_id: row.get(1)?,
                    parent_session_id: row.get(2)?,
                    runtime_session_ref: row.get(3)?,
                    last_assistant_message: row.get(4)?,
                })
            })?;

            let mut sessions = Vec::new();
            for item in mapped {
                sessions.push(item?);
            }
            Ok(sessions)
        })
        .await
        .context("join sqlite load_sessions task failed")??;

        let mut map = HashMap::new();
        for session in rows {
            map.insert(session.session_key.clone(), session);
        }
        Ok(map)
    }

    pub async fn upsert_session(&self, session: &SessionInfo) -> Result<()> {
        let path = self.db_path.clone();
        let session = session.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let conn = Connection::open(&path)
                .with_context(|| format!("open sqlite failed: {:?}", path))?;
            conn.execute(
                r#"
                INSERT INTO core_sessions (
                    session_key, session_id, parent_session_id, runtime_session_ref, last_assistant_message, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                ON CONFLICT(session_key) DO UPDATE SET
                    session_id = excluded.session_id,
                    parent_session_id = excluded.parent_session_id,
                    runtime_session_ref = excluded.runtime_session_ref,
                    last_assistant_message = excluded.last_assistant_message,
                    updated_at = excluded.updated_at
                "#,
                params![
                    session.session_key,
                    session.session_id,
                    session.parent_session_id,
                    session.runtime_session_ref,
                    session.last_assistant_message,
                    now_unix(),
                ],
            )?;
            Ok(())
        })
        .await
        .context("join sqlite upsert_session task failed")??;
        Ok(())
    }

    pub async fn update_session_runtime_state(
        &self,
        session_key: &str,
        runtime_session_ref: Option<&str>,
        last_assistant_message: Option<&str>,
    ) -> Result<()> {
        let path = self.db_path.clone();
        let session_key = session_key.to_string();
        let runtime_session_ref = runtime_session_ref.map(str::to_string);
        let last_assistant_message = last_assistant_message.map(str::to_string);
        tokio::task::spawn_blocking(move || -> Result<()> {
            let conn = Connection::open(&path)
                .with_context(|| format!("open sqlite failed: {:?}", path))?;
            conn.execute(
                r#"
                UPDATE core_sessions
                SET runtime_session_ref = ?2,
                    last_assistant_message = ?3,
                    updated_at = ?4
                WHERE session_key = ?1
                "#,
                params![session_key, runtime_session_ref, last_assistant_message, now_unix()],
            )?;
            Ok(())
        })
        .await
        .context("join sqlite update_session_runtime_state task failed")??;
        Ok(())
    }

    pub async fn create_turn(&self, turn_id: &str, session_id: &str, input_text: &str) -> Result<()> {
        let path = self.db_path.clone();
        let turn_id = turn_id.to_string();
        let session_id = session_id.to_string();
        let input_text = input_text.to_string();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let conn = Connection::open(&path)
                .with_context(|| format!("open sqlite failed: {:?}", path))?;
            let now = now_unix();
            conn.execute(
                r#"
                INSERT INTO core_turns (
                    turn_id, session_id, input_text, status, final_text, error_text, created_at, updated_at
                ) VALUES (?1, ?2, ?3, 'queued', NULL, NULL, ?4, ?4)
                "#,
                params![turn_id, session_id, input_text, now],
            )?;
            Ok(())
        })
        .await
        .context("join sqlite create_turn task failed")??;
        Ok(())
    }

    pub async fn mark_turn_running(&self, turn_id: &str) -> Result<()> {
        self.update_turn_status(turn_id, "running", None, None).await
    }

    pub async fn complete_turn(&self, turn_id: &str, final_text: Option<&str>) -> Result<()> {
        self.update_turn_status(turn_id, "completed", final_text, None).await
    }

    pub async fn fail_turn(&self, turn_id: &str, error_text: &str) -> Result<()> {
        self.update_turn_status(turn_id, "failed", None, Some(error_text)).await
    }

    pub async fn get_turn(&self, turn_id: &str) -> Result<Option<TurnRecord>> {
        let path = self.db_path.clone();
        let turn_id = turn_id.to_string();
        let record = tokio::task::spawn_blocking(move || -> Result<Option<TurnRecord>> {
            let conn = Connection::open(&path)
                .with_context(|| format!("open sqlite failed: {:?}", path))?;
            let mut stmt = conn.prepare(
                "SELECT turn_id, session_id, input_text, status, final_text, error_text FROM core_turns WHERE turn_id = ?1",
            )?;
            let mut rows = stmt.query(params![turn_id])?;
            if let Some(row) = rows.next()? {
                Ok(Some(TurnRecord {
                    turn_id: row.get(0)?,
                    session_id: row.get(1)?,
                    input_text: row.get(2)?,
                    status: row.get(3)?,
                    final_text: row.get(4)?,
                    error_text: row.get(5)?,
                }))
            } else {
                Ok(None)
            }
        })
        .await
        .context("join sqlite get_turn task failed")??;
        Ok(record)
    }

    pub async fn get_active_runtime(&self, session_key: &str) -> Result<Option<RuntimeInstance>> {
        let path = self.db_path.clone();
        let session_key = session_key.to_string();
        let runtime = tokio::task::spawn_blocking(move || -> Result<Option<RuntimeInstance>> {
            let conn = Connection::open(&path)
                .with_context(|| format!("open sqlite failed: {:?}", path))?;
            let mut stmt = conn.prepare(
                r#"
                SELECT r.runtime_id, r.session_key, r.label, r.agent_kind, r.workspace_path,
                       r.runtime_session_ref, r.tag, r.prompt_preview, r.last_assistant_message
                FROM conversation_bindings b
                JOIN runtime_instances r ON r.runtime_id = b.active_runtime_id
                WHERE b.session_key = ?1
                "#,
            )?;
            let mut rows = stmt.query(params![session_key])?;
            if let Some(row) = rows.next()? {
                Ok(Some(RuntimeInstance {
                    runtime_id: row.get(0)?,
                    session_key: row.get(1)?,
                    label: row.get(2)?,
                    agent_kind: row.get(3)?,
                    workspace_path: row.get(4)?,
                    runtime_session_ref: row.get(5)?,
                    tag: row.get(6)?,
                    prompt_preview: row.get(7)?,
                    last_assistant_message: row.get(8)?,
                    is_active: true,
                }))
            } else {
                Ok(None)
            }
        })
        .await
        .context("join sqlite get_active_runtime task failed")??;
        Ok(runtime)
    }

    pub async fn list_runtimes(&self, session_key: &str) -> Result<Vec<RuntimeInstance>> {
        let path = self.db_path.clone();
        let session_key = session_key.to_string();
        let runtimes = tokio::task::spawn_blocking(move || -> Result<Vec<RuntimeInstance>> {
            let conn = Connection::open(&path)
                .with_context(|| format!("open sqlite failed: {:?}", path))?;
            let mut stmt = conn.prepare(
                r#"
                SELECT r.runtime_id, r.session_key, r.label, r.agent_kind, r.workspace_path,
                       r.runtime_session_ref, r.tag, r.prompt_preview, r.last_assistant_message,
                       CASE WHEN b.active_runtime_id = r.runtime_id THEN 1 ELSE 0 END AS is_active
                FROM runtime_instances r
                LEFT JOIN conversation_bindings b ON b.session_key = r.session_key
                WHERE r.session_key = ?1
                ORDER BY is_active DESC, r.created_at DESC
                "#,
            )?;
            let mapped = stmt.query_map(params![session_key], |row| {
                Ok(RuntimeInstance {
                    runtime_id: row.get(0)?,
                    session_key: row.get(1)?,
                    label: row.get(2)?,
                    agent_kind: row.get(3)?,
                    workspace_path: row.get(4)?,
                    runtime_session_ref: row.get(5)?,
                    tag: row.get(6)?,
                    prompt_preview: row.get(7)?,
                    last_assistant_message: row.get(8)?,
                    is_active: row.get::<_, i64>(9)? == 1,
                })
            })?;
            let mut runtimes = Vec::new();
            for item in mapped {
                runtimes.push(item?);
            }
            Ok(runtimes)
        })
        .await
        .context("join sqlite list_runtimes task failed")??;
        Ok(runtimes)
    }

    pub async fn create_runtime(
        &self,
        session_key: &str,
        label: &str,
        agent_kind: &str,
        workspace_path: &str,
        activate: bool,
    ) -> Result<RuntimeInstance> {
        let path = self.db_path.clone();
        let session_key = session_key.to_string();
        let label = label.to_string();
        let agent_kind = agent_kind.to_string();
        let workspace_path = workspace_path.to_string();
        let runtime_id = format!("rt_{}", Uuid::new_v4().simple());
        let runtime_id_for_insert = runtime_id.clone();
        let session_key_for_insert = session_key.clone();
        let label_for_insert = label.clone();
        let agent_kind_for_insert = agent_kind.clone();
        let workspace_path_for_insert = workspace_path.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let conn = Connection::open(&path)
                .with_context(|| format!("open sqlite failed: {:?}", path))?;
            let now = now_unix();
            conn.execute(
                r#"
                INSERT INTO runtime_instances (
                    runtime_id, session_key, label, agent_kind, workspace_path,
                    runtime_session_ref, tag, prompt_preview, last_assistant_message, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, NULL, NULL, NULL, NULL, ?6, ?6)
                "#,
                params![
                    runtime_id_for_insert,
                    session_key_for_insert,
                    label_for_insert,
                    agent_kind_for_insert,
                    workspace_path_for_insert,
                    now
                ],
            )?;
            if activate {
                conn.execute(
                    r#"
                    INSERT INTO conversation_bindings (session_key, active_runtime_id, updated_at)
                    VALUES (?1, ?2, ?3)
                    ON CONFLICT(session_key) DO UPDATE SET
                        active_runtime_id = excluded.active_runtime_id,
                        updated_at = excluded.updated_at
                    "#,
                    params![session_key_for_insert, runtime_id_for_insert, now],
                )?;
            }
            Ok(())
        })
        .await
        .context("join sqlite create_runtime task failed")??;

        Ok(RuntimeInstance {
            runtime_id,
            session_key: session_key.to_string(),
            label: label.to_string(),
            agent_kind: agent_kind.to_string(),
            workspace_path: workspace_path.to_string(),
            runtime_session_ref: None,
            tag: None,
            prompt_preview: None,
            last_assistant_message: None,
            is_active: activate,
        })
    }

    pub async fn import_runtime(
        &self,
        session_key: &str,
        label: &str,
        agent_kind: &str,
        workspace_path: &str,
        runtime_session_ref: &str,
        tag: Option<&str>,
        prompt_preview: Option<&str>,
        activate: bool,
    ) -> Result<RuntimeInstance> {
        let path = self.db_path.clone();
        let session_key = session_key.to_string();
        let label = label.to_string();
        let agent_kind = agent_kind.to_string();
        let workspace_path = workspace_path.to_string();
        let runtime_session_ref = runtime_session_ref.to_string();
        let tag = tag.map(str::to_string);
        let prompt_preview = prompt_preview.map(str::to_string);
        let runtime = tokio::task::spawn_blocking(move || -> Result<RuntimeInstance> {
            let conn = Connection::open(&path)
                .with_context(|| format!("open sqlite failed: {:?}", path))?;
            let now = now_unix();

            let existing = {
                let mut stmt = conn.prepare(
                    r#"
                    SELECT runtime_id
                    FROM runtime_instances
                    WHERE session_key = ?1 AND runtime_session_ref = ?2
                    LIMIT 1
                    "#,
                )?;
                let mut rows = stmt.query(params![session_key, runtime_session_ref])?;
                if let Some(row) = rows.next()? {
                    Some(row.get::<_, String>(0)?)
                } else {
                    None
                }
            };

            let runtime_id = existing.unwrap_or_else(|| format!("rt_{}", Uuid::new_v4().simple()));
            conn.execute(
                r#"
                INSERT INTO runtime_instances (
                    runtime_id, session_key, label, agent_kind, workspace_path,
                    runtime_session_ref, tag, prompt_preview, last_assistant_message, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, ?9, ?9)
                ON CONFLICT(runtime_id) DO UPDATE SET
                    label = excluded.label,
                    agent_kind = excluded.agent_kind,
                    workspace_path = excluded.workspace_path,
                    runtime_session_ref = excluded.runtime_session_ref,
                    tag = excluded.tag,
                    prompt_preview = excluded.prompt_preview,
                    updated_at = excluded.updated_at
                "#,
                params![
                    runtime_id,
                    session_key,
                    label,
                    agent_kind,
                    workspace_path,
                    runtime_session_ref,
                    tag,
                    prompt_preview,
                    now
                ],
            )?;

            if activate {
                conn.execute(
                    r#"
                    INSERT INTO conversation_bindings (session_key, active_runtime_id, updated_at)
                    VALUES (?1, ?2, ?3)
                    ON CONFLICT(session_key) DO UPDATE SET
                        active_runtime_id = excluded.active_runtime_id,
                        updated_at = excluded.updated_at
                    "#,
                    params![session_key, runtime_id, now],
                )?;
            }

            Ok(RuntimeInstance {
                runtime_id,
                session_key,
                label,
                agent_kind,
                workspace_path,
                runtime_session_ref: Some(runtime_session_ref),
                tag,
                prompt_preview,
                last_assistant_message: None,
                is_active: activate,
            })
        })
        .await
        .context("join sqlite import_runtime task failed")??;

        Ok(runtime)
    }

    pub async fn set_active_runtime(&self, session_key: &str, runtime_id: &str) -> Result<()> {
        let path = self.db_path.clone();
        let session_key = session_key.to_string();
        let runtime_id = runtime_id.to_string();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let conn = Connection::open(&path)
                .with_context(|| format!("open sqlite failed: {:?}", path))?;
            conn.execute(
                r#"
                INSERT INTO conversation_bindings (session_key, active_runtime_id, updated_at)
                VALUES (?1, ?2, ?3)
                ON CONFLICT(session_key) DO UPDATE SET
                    active_runtime_id = excluded.active_runtime_id,
                    updated_at = excluded.updated_at
                "#,
                params![session_key, runtime_id, now_unix()],
            )?;
            Ok(())
        })
        .await
        .context("join sqlite set_active_runtime task failed")??;
        Ok(())
    }

    pub async fn update_runtime_state(
        &self,
        runtime_id: &str,
        runtime_session_ref: Option<&str>,
        last_assistant_message: Option<&str>,
    ) -> Result<()> {
        let path = self.db_path.clone();
        let runtime_id = runtime_id.to_string();
        let runtime_session_ref = runtime_session_ref.map(str::to_string);
        let last_assistant_message = last_assistant_message.map(str::to_string);
        tokio::task::spawn_blocking(move || -> Result<()> {
            let conn = Connection::open(&path)
                .with_context(|| format!("open sqlite failed: {:?}", path))?;
            conn.execute(
                r#"
                UPDATE runtime_instances
                SET runtime_session_ref = COALESCE(?2, runtime_session_ref),
                    last_assistant_message = COALESCE(?3, last_assistant_message),
                    updated_at = ?4
                WHERE runtime_id = ?1
                "#,
                params![runtime_id, runtime_session_ref, last_assistant_message, now_unix()],
            )?;
            Ok(())
        })
        .await
        .context("join sqlite update_runtime_state task failed")??;
        Ok(())
    }

    pub async fn update_runtime_workspace(&self, runtime_id: &str, workspace_path: &str) -> Result<()> {
        let path = self.db_path.clone();
        let runtime_id = runtime_id.to_string();
        let workspace_path = workspace_path.to_string();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let conn = Connection::open(&path)
                .with_context(|| format!("open sqlite failed: {:?}", path))?;
            conn.execute(
                r#"
                UPDATE runtime_instances
                SET workspace_path = ?2,
                    updated_at = ?3
                WHERE runtime_id = ?1
                "#,
                params![runtime_id, workspace_path, now_unix()],
            )?;
            Ok(())
        })
        .await
        .context("join sqlite update_runtime_workspace task failed")??;
        Ok(())
    }

    async fn update_turn_status(
        &self,
        turn_id: &str,
        status: &str,
        final_text: Option<&str>,
        error_text: Option<&str>,
    ) -> Result<()> {
        let path = self.db_path.clone();
        let turn_id = turn_id.to_string();
        let status = status.to_string();
        let final_text = final_text.map(str::to_string);
        let error_text = error_text.map(str::to_string);
        tokio::task::spawn_blocking(move || -> Result<()> {
            let conn = Connection::open(&path)
                .with_context(|| format!("open sqlite failed: {:?}", path))?;
            conn.execute(
                r#"
                UPDATE core_turns
                SET status = ?2,
                    final_text = COALESCE(?3, final_text),
                    error_text = COALESCE(?4, error_text),
                    updated_at = ?5
                WHERE turn_id = ?1
                "#,
                params![turn_id, status, final_text, error_text, now_unix()],
            )?;
            Ok(())
        })
        .await
        .context("join sqlite update_turn_status task failed")??;
        Ok(())
    }
}
