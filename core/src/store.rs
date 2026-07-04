//! Local SQLite mail store. Phase 3 (sync) fills it; Phase 4 (UI) reads it.
//! `SqliteMailStore` is the real backend; `MemoryMailStore` backs tests —
//! the same trait seam as `auth::token_store`. The full-text index arrives
//! in Phase 6 (the bundled SQLite build already includes FTS5).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension, Row};
use rusqlite_migration::{Migrations, M};
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum MailStoreError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("migration error: {0}")]
    Migration(#[from] rusqlite_migration::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Folder {
    pub id: String,
    pub display_name: String,
    pub well_known_name: Option<String>,
    pub parent_id: Option<String>,
}

/// The cheap projection for list views — everything but the body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MessageSummary {
    pub id: String,
    pub folder_id: String,
    pub subject: String,
    pub from_name: String,
    pub from_address: String,
    /// Unix epoch seconds (sortable).
    pub received_at: i64,
    pub preview: String,
    pub is_read: bool,
    pub has_attachments: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Message {
    pub summary: MessageSummary,
    /// Stored for forward compatibility; unused until threading.
    pub conversation_id: Option<String>,
    /// ALWAYS sanitized before storage (see `crate::sanitize`) — raw
    /// message HTML never lands in the database.
    pub body_html: Option<String>,
    /// "html" or "text"; `None` until the body has been fetched.
    pub body_content_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SyncState {
    pub folder_id: String,
    /// Graph delta cursor (populated in Phase 3).
    pub delta_link: Option<String>,
    pub last_synced_at: Option<i64>,
}

/// Local mail storage.
///
/// Blocking by design (call via `spawn_blocking` from async code, like the
/// keychain in `auth`). `messages.folder_id` has a foreign key to `folders`,
/// so callers must upsert folders before their messages.
pub trait MailStore: Send + Sync {
    fn upsert_folders(&self, folders: &[Folder]) -> Result<(), MailStoreError>;
    fn list_folders(&self) -> Result<Vec<Folder>, MailStoreError>;
    /// Idempotent by message id. A message whose `body_html` is `None` never
    /// clears an already-stored body (delta re-upserts carry no body).
    fn upsert_messages(&self, messages: &[Message]) -> Result<(), MailStoreError>;
    /// Newest-first within a folder.
    fn list_messages(
        &self,
        folder_id: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<MessageSummary>, MailStoreError>;
    fn get_message(&self, id: &str) -> Result<Option<Message>, MailStoreError>;
    /// Deleting a nonexistent id is a no-op (delta rounds may replay).
    fn delete_message(&self, id: &str) -> Result<(), MailStoreError>;
    /// Cache a lazily-fetched (and already-sanitized) body.
    fn set_message_body(
        &self,
        id: &str,
        body: &str,
        content_type: &str,
    ) -> Result<(), MailStoreError>;
    /// Unread message count per folder id.
    fn unread_counts(&self) -> Result<Vec<(String, u32)>, MailStoreError>;
    /// Whether remote images are allowed for this sender (default false).
    fn get_sender_pref(&self, address: &str) -> Result<bool, MailStoreError>;
    fn set_sender_pref(
        &self,
        address: &str,
        allow_remote_images: bool,
    ) -> Result<(), MailStoreError>;
    fn get_sync_state(&self, folder_id: &str) -> Result<Option<SyncState>, MailStoreError>;
    fn set_sync_state(&self, state: &SyncState) -> Result<(), MailStoreError>;
    /// Bump autocomplete history for successfully-sent recipients.
    fn record_recipients(
        &self,
        recipients: &[(String, String)],
        now: i64,
    ) -> Result<(), MailStoreError>;
    /// Ranked (address, name) suggestions from send history + received-from
    /// frequency. Purely local — works offline.
    fn search_recipients(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<(String, String)>, MailStoreError>;
    fn get_signature(&self, account: &str, kind: &str) -> Result<Option<String>, MailStoreError>;
    fn set_signature(&self, account: &str, kind: &str, content: &str)
        -> Result<(), MailStoreError>;
}

fn migrations() -> Migrations<'static> {
    Migrations::new(vec![
        M::up(
            "CREATE TABLE folders (
            id              TEXT PRIMARY KEY,
            display_name    TEXT NOT NULL,
            well_known_name TEXT,
            parent_id       TEXT
        );
        CREATE TABLE messages (
            id              TEXT PRIMARY KEY,
            folder_id       TEXT NOT NULL REFERENCES folders(id),
            conversation_id TEXT,
            subject         TEXT NOT NULL DEFAULT '',
            from_name       TEXT NOT NULL DEFAULT '',
            from_address    TEXT NOT NULL DEFAULT '',
            received_at     INTEGER NOT NULL,
            preview         TEXT NOT NULL DEFAULT '',
            is_read         INTEGER NOT NULL DEFAULT 0,
            has_attachments INTEGER NOT NULL DEFAULT 0,
            body_html       TEXT
        );
        CREATE INDEX idx_messages_folder_received
            ON messages (folder_id, received_at DESC);
        CREATE TABLE sync_state (
            folder_id      TEXT PRIMARY KEY,
            delta_link     TEXT,
            last_synced_at INTEGER
        );",
        ),
        // v2 (Phase 4): lazy-loaded body metadata + per-sender image prefs.
        M::up(
            "ALTER TABLE messages ADD COLUMN body_content_type TEXT;
        CREATE TABLE sender_prefs (
            address             TEXT PRIMARY KEY,
            allow_remote_images INTEGER NOT NULL DEFAULT 0
        );",
        ),
        // v3 (Phase 5): local autocomplete history + client-side signatures.
        M::up(
            "CREATE TABLE contacts (
            address   TEXT PRIMARY KEY,
            name      TEXT NOT NULL DEFAULT '',
            uses      INTEGER NOT NULL DEFAULT 0,
            last_used INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE signatures (
            account TEXT NOT NULL,
            kind    TEXT NOT NULL,
            content TEXT NOT NULL,
            PRIMARY KEY (account, kind)
        );",
        ),
    ])
}

pub struct SqliteMailStore {
    conn: Mutex<Connection>,
}

impl SqliteMailStore {
    /// Open (creating if needed) the store at `path` and run any pending
    /// migrations. Running against an up-to-date database is a no-op.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, MailStoreError> {
        let path = path.into();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut conn = Connection::open(&path)?;
        Self::init(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// In-memory database with the real schema — used by tests to exercise
    /// the SQL paths without touching disk.
    pub fn open_in_memory() -> Result<Self, MailStoreError> {
        let mut conn = Connection::open_in_memory()?;
        Self::init(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn init(conn: &mut Connection) -> Result<(), MailStoreError> {
        // journal_mode returns a row, so it can't go through pragma_update.
        let _: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        migrations().to_latest(conn)?;
        Ok(())
    }

    fn summary_from_row(row: &Row) -> rusqlite::Result<MessageSummary> {
        Ok(MessageSummary {
            id: row.get("id")?,
            folder_id: row.get("folder_id")?,
            subject: row.get("subject")?,
            from_name: row.get("from_name")?,
            from_address: row.get("from_address")?,
            received_at: row.get("received_at")?,
            preview: row.get("preview")?,
            is_read: row.get("is_read")?,
            has_attachments: row.get("has_attachments")?,
        })
    }
}

impl MailStore for SqliteMailStore {
    fn upsert_folders(&self, folders: &[Folder]) -> Result<(), MailStoreError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO folders (id, display_name, well_known_name, parent_id)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(id) DO UPDATE SET
                     display_name = excluded.display_name,
                     well_known_name = excluded.well_known_name,
                     parent_id = excluded.parent_id",
            )?;
            for f in folders {
                stmt.execute(params![
                    f.id,
                    f.display_name,
                    f.well_known_name,
                    f.parent_id
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn list_folders(&self) -> Result<Vec<Folder>, MailStoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, display_name, well_known_name, parent_id FROM folders ORDER BY id",
        )?;
        let folders = stmt
            .query_map([], |row| {
                Ok(Folder {
                    id: row.get(0)?,
                    display_name: row.get(1)?,
                    well_known_name: row.get(2)?,
                    parent_id: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(folders)
    }

    fn upsert_messages(&self, messages: &[Message]) -> Result<(), MailStoreError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO messages (id, folder_id, conversation_id, subject, from_name,
                                       from_address, received_at, preview, is_read,
                                       has_attachments, body_html, body_content_type)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                 ON CONFLICT(id) DO UPDATE SET
                     folder_id       = excluded.folder_id,
                     conversation_id = excluded.conversation_id,
                     subject         = excluded.subject,
                     from_name       = excluded.from_name,
                     from_address    = excluded.from_address,
                     received_at     = excluded.received_at,
                     preview         = excluded.preview,
                     is_read         = excluded.is_read,
                     has_attachments = excluded.has_attachments,
                     body_html       = COALESCE(excluded.body_html, messages.body_html),
                     body_content_type =
                         COALESCE(excluded.body_content_type, messages.body_content_type)",
            )?;
            for m in messages {
                let s = &m.summary;
                stmt.execute(params![
                    s.id,
                    s.folder_id,
                    m.conversation_id,
                    s.subject,
                    s.from_name,
                    s.from_address,
                    s.received_at,
                    s.preview,
                    s.is_read,
                    s.has_attachments,
                    m.body_html,
                    m.body_content_type,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn list_messages(
        &self,
        folder_id: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<MessageSummary>, MailStoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, folder_id, subject, from_name, from_address, received_at,
                    preview, is_read, has_attachments
             FROM messages
             WHERE folder_id = ?1
             ORDER BY received_at DESC
             LIMIT ?2 OFFSET ?3",
        )?;
        let messages = stmt
            .query_map(params![folder_id, limit, offset], Self::summary_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(messages)
    }

    fn get_message(&self, id: &str) -> Result<Option<Message>, MailStoreError> {
        let conn = self.conn.lock().unwrap();
        let message = conn
            .query_row(
                "SELECT id, folder_id, subject, from_name, from_address, received_at,
                        preview, is_read, has_attachments, conversation_id, body_html,
                        body_content_type
                 FROM messages WHERE id = ?1",
                params![id],
                |row| {
                    Ok(Message {
                        summary: Self::summary_from_row(row)?,
                        conversation_id: row.get("conversation_id")?,
                        body_html: row.get("body_html")?,
                        body_content_type: row.get("body_content_type")?,
                    })
                },
            )
            .optional()?;
        Ok(message)
    }

    fn delete_message(&self, id: &str) -> Result<(), MailStoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM messages WHERE id = ?1", params![id])?;
        Ok(())
    }

    fn set_message_body(
        &self,
        id: &str,
        body: &str,
        content_type: &str,
    ) -> Result<(), MailStoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE messages SET body_html = ?2, body_content_type = ?3 WHERE id = ?1",
            params![id, body, content_type],
        )?;
        Ok(())
    }

    fn unread_counts(&self) -> Result<Vec<(String, u32)>, MailStoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT folder_id, COUNT(*) FROM messages WHERE is_read = 0 GROUP BY folder_id",
        )?;
        let counts = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(counts)
    }

    fn get_sender_pref(&self, address: &str) -> Result<bool, MailStoreError> {
        let conn = self.conn.lock().unwrap();
        let allow: Option<bool> = conn
            .query_row(
                "SELECT allow_remote_images FROM sender_prefs WHERE address = ?1",
                params![address],
                |row| row.get(0),
            )
            .optional()?;
        Ok(allow.unwrap_or(false))
    }

    fn set_sender_pref(
        &self,
        address: &str,
        allow_remote_images: bool,
    ) -> Result<(), MailStoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO sender_prefs (address, allow_remote_images) VALUES (?1, ?2)
             ON CONFLICT(address) DO UPDATE SET allow_remote_images = excluded.allow_remote_images",
            params![address, allow_remote_images],
        )?;
        Ok(())
    }

    fn get_sync_state(&self, folder_id: &str) -> Result<Option<SyncState>, MailStoreError> {
        let conn = self.conn.lock().unwrap();
        let state = conn
            .query_row(
                "SELECT folder_id, delta_link, last_synced_at FROM sync_state WHERE folder_id = ?1",
                params![folder_id],
                |row| {
                    Ok(SyncState {
                        folder_id: row.get(0)?,
                        delta_link: row.get(1)?,
                        last_synced_at: row.get(2)?,
                    })
                },
            )
            .optional()?;
        Ok(state)
    }

    fn set_sync_state(&self, state: &SyncState) -> Result<(), MailStoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO sync_state (folder_id, delta_link, last_synced_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(folder_id) DO UPDATE SET
                 delta_link = excluded.delta_link,
                 last_synced_at = excluded.last_synced_at",
            params![state.folder_id, state.delta_link, state.last_synced_at],
        )?;
        Ok(())
    }

    fn record_recipients(
        &self,
        recipients: &[(String, String)],
        now: i64,
    ) -> Result<(), MailStoreError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO contacts (address, name, uses, last_used) VALUES (?1, ?2, 1, ?3)
                 ON CONFLICT(address) DO UPDATE SET
                     uses = uses + 1,
                     last_used = excluded.last_used,
                     name = CASE WHEN excluded.name <> '' THEN excluded.name
                                 ELSE contacts.name END",
            )?;
            for (address, name) in recipients {
                if !address.is_empty() {
                    stmt.execute(params![address.to_lowercase(), name, now])?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn search_recipients(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<(String, String)>, MailStoreError> {
        let pattern = format!(
            "%{}%",
            query
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        let conn = self.conn.lock().unwrap();
        // Explicitly-used addresses (contacts) rank above mere senders.
        let mut stmt = conn.prepare(
            "SELECT address, MAX(name) AS name, SUM(score) AS total FROM (
                 SELECT address, name, uses * 10 AS score FROM contacts
                  WHERE address LIKE ?1 ESCAPE '\\' OR name LIKE ?1 ESCAPE '\\'
                 UNION ALL
                 SELECT lower(from_address), from_name, COUNT(*) FROM messages
                  WHERE from_address <> ''
                    AND (from_address LIKE ?1 ESCAPE '\\' OR from_name LIKE ?1 ESCAPE '\\')
                  GROUP BY lower(from_address), from_name
             )
             GROUP BY address
             ORDER BY total DESC, address
             LIMIT ?2",
        )?;
        let results = stmt
            .query_map(params![pattern, limit], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(results)
    }

    fn get_signature(&self, account: &str, kind: &str) -> Result<Option<String>, MailStoreError> {
        let conn = self.conn.lock().unwrap();
        let content = conn
            .query_row(
                "SELECT content FROM signatures WHERE account = ?1 AND kind = ?2",
                params![account, kind],
                |row| row.get(0),
            )
            .optional()?;
        Ok(content)
    }

    fn set_signature(
        &self,
        account: &str,
        kind: &str,
        content: &str,
    ) -> Result<(), MailStoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO signatures (account, kind, content) VALUES (?1, ?2, ?3)
             ON CONFLICT(account, kind) DO UPDATE SET content = excluded.content",
            params![account, kind, content],
        )?;
        Ok(())
    }
}

/// In-memory store for tests. Mirrors `SqliteMailStore` semantics, including
/// body preservation on summary-only upserts.
#[derive(Default)]
pub struct MemoryMailStore {
    folders: Mutex<HashMap<String, Folder>>,
    messages: Mutex<HashMap<String, Message>>,
    sync_state: Mutex<HashMap<String, SyncState>>,
    sender_prefs: Mutex<HashMap<String, bool>>,
    /// address → (name, uses, last_used)
    contacts: Mutex<HashMap<String, (String, u32, i64)>>,
    /// (account, kind) → content
    signatures: Mutex<HashMap<(String, String), String>>,
}

impl MailStore for MemoryMailStore {
    fn upsert_folders(&self, folders: &[Folder]) -> Result<(), MailStoreError> {
        let mut map = self.folders.lock().unwrap();
        for f in folders {
            map.insert(f.id.clone(), f.clone());
        }
        Ok(())
    }

    fn list_folders(&self) -> Result<Vec<Folder>, MailStoreError> {
        let map = self.folders.lock().unwrap();
        let mut folders: Vec<_> = map.values().cloned().collect();
        folders.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(folders)
    }

    fn upsert_messages(&self, messages: &[Message]) -> Result<(), MailStoreError> {
        let mut map = self.messages.lock().unwrap();
        for m in messages {
            let mut m = m.clone();
            if let Some(existing) = map.get(&m.summary.id) {
                if m.body_html.is_none() {
                    m.body_html = existing.body_html.clone();
                }
                if m.body_content_type.is_none() {
                    m.body_content_type = existing.body_content_type.clone();
                }
            }
            map.insert(m.summary.id.clone(), m);
        }
        Ok(())
    }

    fn list_messages(
        &self,
        folder_id: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<MessageSummary>, MailStoreError> {
        let map = self.messages.lock().unwrap();
        let mut summaries: Vec<_> = map
            .values()
            .filter(|m| m.summary.folder_id == folder_id)
            .map(|m| m.summary.clone())
            .collect();
        summaries.sort_by_key(|s| std::cmp::Reverse(s.received_at));
        Ok(summaries
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .collect())
    }

    fn get_message(&self, id: &str) -> Result<Option<Message>, MailStoreError> {
        Ok(self.messages.lock().unwrap().get(id).cloned())
    }

    fn delete_message(&self, id: &str) -> Result<(), MailStoreError> {
        self.messages.lock().unwrap().remove(id);
        Ok(())
    }

    fn set_message_body(
        &self,
        id: &str,
        body: &str,
        content_type: &str,
    ) -> Result<(), MailStoreError> {
        if let Some(m) = self.messages.lock().unwrap().get_mut(id) {
            m.body_html = Some(body.to_string());
            m.body_content_type = Some(content_type.to_string());
        }
        Ok(())
    }

    fn unread_counts(&self) -> Result<Vec<(String, u32)>, MailStoreError> {
        let map = self.messages.lock().unwrap();
        let mut counts: HashMap<String, u32> = HashMap::new();
        for m in map.values().filter(|m| !m.summary.is_read) {
            *counts.entry(m.summary.folder_id.clone()).or_default() += 1;
        }
        let mut counts: Vec<_> = counts.into_iter().collect();
        counts.sort();
        Ok(counts)
    }

    fn get_sender_pref(&self, address: &str) -> Result<bool, MailStoreError> {
        Ok(self
            .sender_prefs
            .lock()
            .unwrap()
            .get(address)
            .copied()
            .unwrap_or(false))
    }

    fn set_sender_pref(
        &self,
        address: &str,
        allow_remote_images: bool,
    ) -> Result<(), MailStoreError> {
        self.sender_prefs
            .lock()
            .unwrap()
            .insert(address.to_string(), allow_remote_images);
        Ok(())
    }

    fn get_sync_state(&self, folder_id: &str) -> Result<Option<SyncState>, MailStoreError> {
        Ok(self.sync_state.lock().unwrap().get(folder_id).cloned())
    }

    fn set_sync_state(&self, state: &SyncState) -> Result<(), MailStoreError> {
        self.sync_state
            .lock()
            .unwrap()
            .insert(state.folder_id.clone(), state.clone());
        Ok(())
    }

    fn record_recipients(
        &self,
        recipients: &[(String, String)],
        now: i64,
    ) -> Result<(), MailStoreError> {
        let mut map = self.contacts.lock().unwrap();
        for (address, name) in recipients {
            if address.is_empty() {
                continue;
            }
            let entry = map
                .entry(address.to_lowercase())
                .or_insert_with(|| (String::new(), 0, 0));
            entry.1 += 1;
            entry.2 = now;
            if !name.is_empty() {
                entry.0 = name.clone();
            }
        }
        Ok(())
    }

    fn search_recipients(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<(String, String)>, MailStoreError> {
        let q = query.to_lowercase();
        let matches = |addr: &str, name: &str| {
            addr.to_lowercase().contains(&q) || name.to_lowercase().contains(&q)
        };
        // address → (name, score)
        let mut scored: HashMap<String, (String, i64)> = HashMap::new();
        for (address, (name, uses, _)) in self.contacts.lock().unwrap().iter() {
            if matches(address, name) {
                let e = scored.entry(address.clone()).or_default();
                e.0 = name.clone();
                e.1 += i64::from(*uses) * 10;
            }
        }
        for m in self.messages.lock().unwrap().values() {
            let addr = m.summary.from_address.to_lowercase();
            if !addr.is_empty() && matches(&addr, &m.summary.from_name) {
                let e = scored.entry(addr).or_default();
                if e.0.is_empty() {
                    e.0 = m.summary.from_name.clone();
                }
                e.1 += 1;
            }
        }
        let mut results: Vec<_> = scored
            .into_iter()
            .map(|(addr, (name, score))| (addr, name, score))
            .collect();
        results.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
        Ok(results
            .into_iter()
            .take(limit as usize)
            .map(|(addr, name, _)| (addr, name))
            .collect())
    }

    fn get_signature(&self, account: &str, kind: &str) -> Result<Option<String>, MailStoreError> {
        Ok(self
            .signatures
            .lock()
            .unwrap()
            .get(&(account.to_string(), kind.to_string()))
            .cloned())
    }

    fn set_signature(
        &self,
        account: &str,
        kind: &str,
        content: &str,
    ) -> Result<(), MailStoreError> {
        self.signatures
            .lock()
            .unwrap()
            .insert((account.to_string(), kind.to_string()), content.to_string());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(id: &str, name: &str) -> Folder {
        Folder {
            id: id.to_string(),
            display_name: name.to_string(),
            well_known_name: None,
            parent_id: None,
        }
    }

    fn message(id: &str, folder_id: &str, received_at: i64, body: Option<&str>) -> Message {
        Message {
            summary: MessageSummary {
                id: id.to_string(),
                folder_id: folder_id.to_string(),
                subject: format!("subject {id}"),
                from_name: "Sender".to_string(),
                from_address: "sender@example.com".to_string(),
                received_at,
                preview: format!("preview {id}"),
                is_read: false,
                has_attachments: false,
            },
            conversation_id: None,
            body_html: body.map(str::to_string),
            body_content_type: body.map(|_| "html".to_string()),
        }
    }

    /// The shared contract both backends must satisfy.
    fn exercise(store: &dyn MailStore) {
        // Folders round-trip; re-upsert updates rather than duplicates.
        store
            .upsert_folders(&[folder("f1", "Inbox"), folder("f2", "Archive")])
            .unwrap();
        store
            .upsert_folders(&[folder("f1", "Inbox (renamed)")])
            .unwrap();
        let folders = store.list_folders().unwrap();
        assert_eq!(folders.len(), 2);
        assert_eq!(folders[0].display_name, "Inbox (renamed)");

        // Messages: newest-first ordering with limit/offset.
        store
            .upsert_messages(&[
                message("m1", "f1", 100, Some("<p>old</p>")),
                message("m2", "f1", 300, None),
                message("m3", "f1", 200, None),
                message("m4", "f2", 999, None),
            ])
            .unwrap();
        let page = store.list_messages("f1", 2, 0).unwrap();
        assert_eq!(
            page.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["m2", "m3"]
        );
        let rest = store.list_messages("f1", 10, 2).unwrap();
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].id, "m1");

        // Full record incl. body.
        let m1 = store.get_message("m1").unwrap().unwrap();
        assert_eq!(m1.body_html.as_deref(), Some("<p>old</p>"));
        assert!(store.get_message("nope").unwrap().is_none());

        // Re-upsert same id updates, not duplicates; a summary-only upsert
        // (body None) must preserve the stored body.
        let mut updated = message("m1", "f1", 150, None);
        updated.summary.is_read = true;
        store.upsert_messages(&[updated]).unwrap();
        assert_eq!(store.list_messages("f1", 10, 0).unwrap().len(), 3);
        let m1 = store.get_message("m1").unwrap().unwrap();
        assert!(m1.summary.is_read);
        assert_eq!(m1.summary.received_at, 150);
        assert_eq!(
            m1.body_html.as_deref(),
            Some("<p>old</p>"),
            "summary-only upsert must not clear the body"
        );

        // Lazy body caching: set_message_body fills body + content type;
        // a later summary-only upsert must preserve both.
        store
            .set_message_body("m2", "<p>fetched</p>", "html")
            .unwrap();
        store
            .upsert_messages(&[message("m2", "f1", 300, None)])
            .unwrap();
        let m2 = store.get_message("m2").unwrap().unwrap();
        assert_eq!(m2.body_html.as_deref(), Some("<p>fetched</p>"));
        assert_eq!(m2.body_content_type.as_deref(), Some("html"));

        // Unread counts per folder (m1 was marked read above).
        let counts = store.unread_counts().unwrap();
        assert_eq!(counts, vec![("f1".to_string(), 2), ("f2".to_string(), 1)]);

        // Sender prefs: default false; set + overwrite round-trips.
        assert!(!store.get_sender_pref("a@example.com").unwrap());
        store.set_sender_pref("a@example.com", true).unwrap();
        assert!(store.get_sender_pref("a@example.com").unwrap());
        store.set_sender_pref("a@example.com", false).unwrap();
        assert!(!store.get_sender_pref("a@example.com").unwrap());

        // Recipient history: contact score (uses × 10) outranks a single
        // received-from hit; matching is case-insensitive over name+address.
        store
            .record_recipients(&[("Colleague@Example.com".into(), "Colleague".into())], 100)
            .unwrap();
        store
            .record_recipients(&[("colleague@example.com".into(), String::new())], 200)
            .unwrap();
        let hits = store.search_recipients("colleague", 5).unwrap();
        assert_eq!(hits.len(), 1, "recording twice must not duplicate");
        assert_eq!(
            hits[0],
            ("colleague@example.com".into(), "Colleague".into())
        );
        // The message sender seeded above is also findable, ranked below.
        let hits = store.search_recipients("sender", 5).unwrap();
        assert_eq!(hits[0].0, "sender@example.com");
        assert!(store
            .search_recipients("zzz-no-match", 5)
            .unwrap()
            .is_empty());

        // Signatures: per account + kind, overwrite wins.
        assert!(store.get_signature("me@x.com", "new").unwrap().is_none());
        store.set_signature("me@x.com", "new", "— Me").unwrap();
        store
            .set_signature("me@x.com", "reply", "— Me (reply)")
            .unwrap();
        store.set_signature("me@x.com", "new", "— Me v2").unwrap();
        assert_eq!(
            store.get_signature("me@x.com", "new").unwrap().as_deref(),
            Some("— Me v2")
        );
        assert_eq!(
            store.get_signature("me@x.com", "reply").unwrap().as_deref(),
            Some("— Me (reply)")
        );
        assert!(store.get_signature("other@x.com", "new").unwrap().is_none());

        // Deletion: removes the row; deleting again (or a missing id) is a no-op.
        store
            .upsert_messages(&[message("gone", "f1", 50, None)])
            .unwrap();
        store.delete_message("gone").unwrap();
        assert!(store.get_message("gone").unwrap().is_none());
        store.delete_message("gone").unwrap();
        store.delete_message("never-existed").unwrap();

        // Sync state round-trip and update.
        assert!(store.get_sync_state("f1").unwrap().is_none());
        store
            .set_sync_state(&SyncState {
                folder_id: "f1".to_string(),
                delta_link: Some("https://graph/delta?token=1".to_string()),
                last_synced_at: Some(1_000),
            })
            .unwrap();
        store
            .set_sync_state(&SyncState {
                folder_id: "f1".to_string(),
                delta_link: Some("https://graph/delta?token=2".to_string()),
                last_synced_at: Some(2_000),
            })
            .unwrap();
        let state = store.get_sync_state("f1").unwrap().unwrap();
        assert_eq!(
            state.delta_link.as_deref(),
            Some("https://graph/delta?token=2")
        );
        assert_eq!(state.last_synced_at, Some(2_000));
    }

    #[test]
    fn memory_store_contract() {
        exercise(&MemoryMailStore::default());
    }

    #[test]
    fn sqlite_store_contract() {
        exercise(&SqliteMailStore::open_in_memory().unwrap());
    }

    #[test]
    fn sqlite_creates_file_with_schema_and_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("spite.db");
        let _store = SqliteMailStore::open(&path).unwrap();
        assert!(path.exists());

        let conn = Connection::open(&path).unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 3);
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        for required in [
            "folders",
            "messages",
            "sync_state",
            "sender_prefs",
            "contacts",
            "signatures",
        ] {
            assert!(tables.iter().any(|t| t == required), "missing {required}");
        }
    }

    #[test]
    fn migrations_are_valid() {
        assert!(migrations().validate().is_ok());
    }

    #[test]
    fn sqlite_open_is_idempotent_and_persistent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("spite.db");

        let store = SqliteMailStore::open(&path).unwrap();
        store.upsert_folders(&[folder("f1", "Inbox")]).unwrap();
        drop(store);

        // Second open: migrations are a no-op, data persists.
        let store = SqliteMailStore::open(&path).unwrap();
        let folders = store.list_folders().unwrap();
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].display_name, "Inbox");
    }
}
