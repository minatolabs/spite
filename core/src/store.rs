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

use crate::sanitize::html_to_text;

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
    /// Graph `flag.flagStatus`: notFlagged / flagged / complete.
    pub flag_status: String,
    /// Graph `inferenceClassification`: focused / other.
    pub inference_classification: String,
    pub is_draft: bool,
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
    /// RFC 5322 Message-ID — the dedupe key against server search hits.
    pub internet_message_id: Option<String>,
    /// Assigned categories (Graph `categories`).
    pub categories: Vec<String>,
}

/// Message-list filter chips; also usable without a query (filtered browse).
#[derive(Debug, Clone, Default, Serialize, serde::Deserialize)]
pub struct SearchFilters {
    pub folder_id: Option<String>,
    #[serde(default)]
    pub unread_only: bool,
    #[serde(default)]
    pub has_attachments: bool,
    pub from: Option<String>,
    pub date_from: Option<i64>,
    pub date_to: Option<i64>,
    #[serde(default)]
    pub flagged_only: bool,
}

impl SearchFilters {
    pub fn is_empty(&self) -> bool {
        self.folder_id.is_none()
            && !self.unread_only
            && !self.has_attachments
            && !self.flagged_only
            && self.from.is_none()
            && self.date_from.is_none()
            && self.date_to.is_none()
    }
}

/// One search result. `title`/`snippet` carry match highlighting delimited
/// by the private-use markers below — the UI splits on them and builds
/// `<mark>` DOM nodes, so message text can never inject markup.
#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    pub entity_type: String,
    pub entity_id: String,
    pub title: String,
    pub snippet: String,
    pub ts: i64,
    /// Populated for mail hits; `None` for future entity types.
    pub summary: Option<MessageSummary>,
}

pub const HIGHLIGHT_START: char = '\u{E000}';
pub const HIGHLIGHT_END: char = '\u{E001}';

#[derive(Debug, Clone, Serialize)]
pub struct SavedSearch {
    pub id: i64,
    pub name: String,
    pub query: String,
    /// JSON-serialized `SearchFilters`.
    pub filters: String,
}

/// Turn raw user input into a safe FTS5 MATCH expression: every token is
/// double-quoted (colons, NEAR, AND/OR/NOT and stray quotes become inert)
/// and the final token matches as a prefix for search-as-you-type.
pub fn fts_query(input: &str) -> Option<String> {
    let tokens: Vec<&str> = input.split_whitespace().collect();
    let last = tokens.len().checked_sub(1)?;
    Some(
        tokens
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let escaped = t.replace('"', "\"\"");
                if i == last {
                    format!("\"{escaped}\"*")
                } else {
                    format!("\"{escaped}\"")
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// KQL for Graph `$search` from the same query + filters (server fallback).
pub fn build_kql(query: &str, filters: &SearchFilters) -> String {
    let mut parts = Vec::new();
    let q = query.trim();
    if !q.is_empty() {
        parts.push(q.replace('"', "").to_string());
    }
    if let Some(from) = filters.from.as_deref().filter(|f| !f.trim().is_empty()) {
        parts.push(format!("from:{}", from.trim()));
    }
    if filters.has_attachments {
        parts.push("hasAttachment:true".to_string());
    }
    let fmt_day = |epoch: i64| {
        chrono::DateTime::from_timestamp(epoch, 0)
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default()
    };
    if let Some(d) = filters.date_from {
        parts.push(format!("received>={}", fmt_day(d)));
    }
    if let Some(d) = filters.date_to {
        parts.push(format!("received<={}", fmt_day(d)));
    }
    parts.join(" ")
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
    /// Ranked, highlighted full-text search. An empty/whitespace query with
    /// filters set is a filtered browse (newest first).
    fn search(
        &self,
        query: &str,
        filters: &SearchFilters,
        limit: u32,
    ) -> Result<Vec<SearchHit>, MailStoreError>;
    /// Index a non-mail document (mail rows are trigger-maintained). This is
    /// the seam calendar/contact search plugs into later.
    fn index_document(
        &self,
        entity_type: &str,
        entity_id: &str,
        title: &str,
        subtitle: &str,
        body: &str,
        ts: i64,
    ) -> Result<(), MailStoreError>;
    fn list_saved_searches(&self) -> Result<Vec<SavedSearch>, MailStoreError>;
    fn save_search(
        &self,
        name: &str,
        query: &str,
        filters_json: &str,
    ) -> Result<i64, MailStoreError>;
    fn delete_saved_search(&self, id: i64) -> Result<(), MailStoreError>;
    /// Dedupe helper for server search hits: does this message already exist
    /// locally, by Graph id or by RFC 5322 internetMessageId?
    fn message_exists(
        &self,
        id: &str,
        internet_message_id: Option<&str>,
    ) -> Result<bool, MailStoreError>;

    /// Ids of rows whose summary was blanked by a partial delta event
    /// (empty sender + no received date) — candidates for a re-fetch repair.
    fn broken_summary_ids(&self, limit: u32) -> Result<Vec<String>, MailStoreError>;

    // --- Phase 7 optimistic write mutators. Each is a pure local state
    // change; `ops::execute_op` pairs them with the Graph call + rollback. ---

    fn set_read_state(&self, id: &str, is_read: bool) -> Result<(), MailStoreError>;
    fn set_flag_status(&self, id: &str, flag_status: &str) -> Result<(), MailStoreError>;
    fn set_categories(&self, id: &str, categories: &[String]) -> Result<(), MailStoreError>;
    fn set_inference(&self, id: &str, classification: &str) -> Result<(), MailStoreError>;
    /// Repoint a message at a folder (optimistic move). `new_id` lets a
    /// confirmed Graph move swap in the destination-copy id.
    fn move_message(
        &self,
        id: &str,
        new_folder_id: &str,
        new_id: Option<&str>,
    ) -> Result<(), MailStoreError>;
}

fn migrations() -> Migrations<'static> {
    Migrations::new(migration_steps())
}

fn migration_steps() -> Vec<M<'static>> {
    vec![
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
        // v4: purge Exchange legacy X.500 DNs ('/o=.../cn=...') that leaked
        // into autocomplete before addresses were SMTP-validated. The SQL
        // approximates is_smtp_address(); the Rust filter is authoritative
        // for everything written from here on.
        M::up(
            "DELETE FROM contacts
              WHERE address LIKE '/%'
                 OR address NOT LIKE '%_@_%.%';",
        ),
        // v5 (Phase 6): entity-agnostic FTS5 search index, trigger-maintained
        // for mail (other entity types insert directly via index_document).
        // Bodies are indexed as plain text via the strip_html() SQL function
        // registered in SqliteMailStore::init BEFORE migrations run — which
        // also means tools without that function can't write to `messages`.
        // The delete-by-entity_id in triggers scans the FTS table (entity_id
        // is UNINDEXED): O(n) per message update, fine at v0.1 window sizes;
        // add a rowid map if mailboxes grow.
        //
        // internet_message_id backs server-search dedupe; resetting the
        // delta cursors makes the next sync re-walk each folder window with
        // the widened $select to backfill it (idempotent upserts).
        M::up(
            "ALTER TABLE messages ADD COLUMN internet_message_id TEXT;
        CREATE VIRTUAL TABLE search_index USING fts5(
            entity_type UNINDEXED,
            entity_id   UNINDEXED,
            title,
            subtitle,
            body,
            ts          UNINDEXED,
            tokenize = 'unicode61 remove_diacritics 2',
            prefix = '2 3'
        );
        CREATE TRIGGER messages_search_ai AFTER INSERT ON messages BEGIN
            INSERT INTO search_index (entity_type, entity_id, title, subtitle, body, ts)
            VALUES ('mail', new.id, new.subject,
                    new.from_name || ' ' || new.from_address,
                    CASE WHEN new.body_html IS NULL THEN new.preview
                         WHEN new.body_content_type = 'text' THEN new.body_html
                         ELSE strip_html(new.body_html) END,
                    new.received_at);
        END;
        CREATE TRIGGER messages_search_au AFTER UPDATE ON messages BEGIN
            DELETE FROM search_index WHERE entity_type = 'mail' AND entity_id = old.id;
            INSERT INTO search_index (entity_type, entity_id, title, subtitle, body, ts)
            VALUES ('mail', new.id, new.subject,
                    new.from_name || ' ' || new.from_address,
                    CASE WHEN new.body_html IS NULL THEN new.preview
                         WHEN new.body_content_type = 'text' THEN new.body_html
                         ELSE strip_html(new.body_html) END,
                    new.received_at);
        END;
        CREATE TRIGGER messages_search_ad AFTER DELETE ON messages BEGIN
            DELETE FROM search_index WHERE entity_type = 'mail' AND entity_id = old.id;
        END;
        INSERT INTO search_index (entity_type, entity_id, title, subtitle, body, ts)
        SELECT 'mail', id, subject, from_name || ' ' || from_address,
               CASE WHEN body_html IS NULL THEN preview
                    WHEN body_content_type = 'text' THEN body_html
                    ELSE strip_html(body_html) END,
               received_at
          FROM messages;
        CREATE TABLE saved_searches (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            name       TEXT NOT NULL,
            query      TEXT NOT NULL DEFAULT '',
            filters    TEXT NOT NULL DEFAULT '{}',
            created_at INTEGER NOT NULL DEFAULT 0
        );
        UPDATE sync_state SET delta_link = NULL;",
        ),
        // v6 (Phase 7): mail-management state. Flag/inference/categories/draft
        // arrive with the widened sync $select; the cursor reset re-walks each
        // window so existing rows backfill (idempotent upserts). Existing FTS
        // triggers keep firing — the new columns aren't indexed, so no trigger
        // change is needed.
        M::up(
            "ALTER TABLE messages ADD COLUMN flag_status TEXT NOT NULL DEFAULT 'notFlagged';
        ALTER TABLE messages ADD COLUMN inference_classification TEXT NOT NULL DEFAULT 'focused';
        ALTER TABLE messages ADD COLUMN categories TEXT NOT NULL DEFAULT '[]';
        ALTER TABLE messages ADD COLUMN is_draft INTEGER NOT NULL DEFAULT 0;
        UPDATE sync_state SET delta_link = NULL;",
        ),
    ]
}

/// True only for plausible SMTP addresses: one `@` with a non-empty local
/// part and a dotted domain. Rejects Exchange legacy X.500 DNs
/// (`/o=…/cn=…`), which Graph sometimes returns in `emailAddress.address`.
pub fn is_smtp_address(addr: &str) -> bool {
    if addr.starts_with('/') {
        return false;
    }
    let Some((local, domain)) = addr.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.contains('@')
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
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
        // The search_index triggers (and the v5 backfill) call this, so it
        // must exist before migrations run.
        conn.create_scalar_function(
            "strip_html",
            1,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8
                | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
            |ctx| {
                let html: String = ctx.get(0)?;
                Ok(crate::sanitize::html_to_text(&html))
            },
        )?;
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
            flag_status: row.get("flag_status")?,
            inference_classification: row.get("inference_classification")?,
            is_draft: row.get("is_draft")?,
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
                                       has_attachments, body_html, body_content_type,
                                       internet_message_id, flag_status,
                                       inference_classification, categories, is_draft)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                         ?14, ?15, ?16, ?17)
                 ON CONFLICT(id) DO UPDATE SET
                     folder_id       = excluded.folder_id,
                     -- Incremental delta emits bare change-events (a read/flag
                     -- flip) as a message carrying only its id + the changed
                     -- state, with no from/subject/receivedDateTime. Detect
                     -- that (empty from + received_at 0) and preserve the
                     -- existing descriptive fields, so such an event never
                     -- blanks a good summary. The state flags below still apply.
                     subject = CASE
                         WHEN excluded.from_address = '' AND excluded.received_at = 0
                         THEN messages.subject ELSE excluded.subject END,
                     from_name = CASE
                         WHEN excluded.from_address = '' AND excluded.received_at = 0
                         THEN messages.from_name ELSE excluded.from_name END,
                     from_address = CASE
                         WHEN excluded.from_address = '' AND excluded.received_at = 0
                         THEN messages.from_address ELSE excluded.from_address END,
                     received_at = CASE
                         WHEN excluded.received_at = 0
                         THEN messages.received_at ELSE excluded.received_at END,
                     preview = CASE
                         WHEN excluded.from_address = '' AND excluded.received_at = 0
                         THEN messages.preview ELSE excluded.preview END,
                     has_attachments = CASE
                         WHEN excluded.from_address = '' AND excluded.received_at = 0
                         THEN messages.has_attachments ELSE excluded.has_attachments END,
                     categories = CASE
                         WHEN excluded.from_address = '' AND excluded.received_at = 0
                         THEN messages.categories ELSE excluded.categories END,
                     is_read         = excluded.is_read,
                     body_html       = COALESCE(excluded.body_html, messages.body_html),
                     body_content_type =
                         COALESCE(excluded.body_content_type, messages.body_content_type),
                     conversation_id =
                         COALESCE(excluded.conversation_id, messages.conversation_id),
                     internet_message_id =
                         COALESCE(excluded.internet_message_id, messages.internet_message_id),
                     flag_status              = excluded.flag_status,
                     inference_classification = excluded.inference_classification,
                     is_draft                 = excluded.is_draft",
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
                    m.internet_message_id,
                    s.flag_status,
                    s.inference_classification,
                    serde_json::to_string(&m.categories).unwrap_or_else(|_| "[]".to_string()),
                    s.is_draft,
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
                    preview, is_read, has_attachments, flag_status,
                    inference_classification, is_draft
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
                        preview, is_read, has_attachments, flag_status,
                        inference_classification, is_draft, conversation_id, body_html,
                        body_content_type, internet_message_id, categories
                 FROM messages WHERE id = ?1",
                params![id],
                |row| {
                    Ok(Message {
                        summary: Self::summary_from_row(row)?,
                        conversation_id: row.get("conversation_id")?,
                        body_html: row.get("body_html")?,
                        body_content_type: row.get("body_content_type")?,
                        internet_message_id: row.get("internet_message_id")?,
                        categories: serde_json::from_str(&row.get::<_, String>("categories")?)
                            .unwrap_or_default(),
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
                if is_smtp_address(address) {
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
        // Both branches surface SMTP shapes only — Graph sender addresses
        // can be Exchange legacy DNs, which never belong in suggestions.
        let mut stmt = conn.prepare(
            "SELECT address, MAX(name) AS name, SUM(score) AS total FROM (
                 SELECT address, name, uses * 10 AS score FROM contacts
                  WHERE (address LIKE ?1 ESCAPE '\\' OR name LIKE ?1 ESCAPE '\\')
                    AND address NOT LIKE '/%' AND address LIKE '%_@_%.%'
                 UNION ALL
                 SELECT lower(from_address), from_name, COUNT(*) FROM messages
                  WHERE from_address NOT LIKE '/%' AND from_address LIKE '%_@_%.%'
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

    fn search(
        &self,
        query: &str,
        filters: &SearchFilters,
        limit: u32,
    ) -> Result<Vec<SearchHit>, MailStoreError> {
        use rusqlite::types::Value;

        let conn = self.conn.lock().unwrap();
        let now = crate::sync::now_epoch();

        // Mail-specific filter predicates (applied to the joined messages
        // row for search, or directly for filtered browse).
        let mut preds = String::new();
        let mut pred_params: Vec<Value> = Vec::new();
        if let Some(folder) = &filters.folder_id {
            preds.push_str(" AND m.folder_id = ?");
            pred_params.push(Value::Text(folder.clone()));
        }
        if filters.unread_only {
            preds.push_str(" AND m.is_read = 0");
        }
        if filters.has_attachments {
            preds.push_str(" AND m.has_attachments = 1");
        }
        if filters.flagged_only {
            preds.push_str(" AND m.flag_status = 'flagged'");
        }
        if let Some(from) = filters.from.as_deref().filter(|f| !f.trim().is_empty()) {
            preds.push_str(" AND (m.from_address LIKE ? OR m.from_name LIKE ?)");
            let pat = format!("%{}%", from.trim());
            pred_params.push(Value::Text(pat.clone()));
            pred_params.push(Value::Text(pat));
        }
        if let Some(d) = filters.date_from {
            preds.push_str(" AND m.received_at >= ?");
            pred_params.push(Value::Integer(d));
        }
        if let Some(d) = filters.date_to {
            preds.push_str(" AND m.received_at <= ?");
            pred_params.push(Value::Integer(d));
        }

        let Some(match_expr) = fts_query(query) else {
            // Filtered browse: no FTS, plain newest-first message query.
            let sql = format!(
                "SELECT id, folder_id, subject, from_name, from_address, received_at,
                        preview, is_read, has_attachments, flag_status,
                        inference_classification, is_draft
                   FROM messages m
                  WHERE 1=1{preds}
                  ORDER BY received_at DESC
                  LIMIT ?"
            );
            pred_params.push(Value::Integer(i64::from(limit)));
            let mut stmt = conn.prepare(&sql)?;
            let hits = stmt
                .query_map(rusqlite::params_from_iter(pred_params), |row| {
                    let summary = Self::summary_from_row(row)?;
                    Ok(SearchHit {
                        entity_type: "mail".to_string(),
                        entity_id: summary.id.clone(),
                        title: summary.subject.clone(),
                        snippet: summary.preview.clone(),
                        ts: summary.received_at,
                        summary: Some(summary),
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            return Ok(hits);
        };

        // Mail filters only constrain mail hits; when any is set, non-mail
        // entities are excluded (the filters are mail concepts).
        let entity_guard = if filters.is_empty() {
            ""
        } else {
            " AND s.entity_type = 'mail'"
        };
        // bm25: 6 weights for 6 columns (unindexed ones weighted 0);
        // title > subtitle > body, blended with a mild recency penalty
        // (~0.01 per day, capped at a year) — bm25 is lower-is-better.
        let sql = format!(
            "SELECT s.entity_type, s.entity_id,
                    highlight(search_index, 2, ?2, ?3) AS title_hl,
                    snippet(search_index, 4, ?2, ?3, '…', 12) AS snip,
                    CAST(s.ts AS INTEGER) AS ts_i,
                    m.id, m.folder_id, m.subject, m.from_name, m.from_address,
                    m.received_at, m.preview, m.is_read, m.has_attachments,
                    m.flag_status, m.inference_classification, m.is_draft,
                    bm25(search_index, 0.0, 0.0, 4.0, 2.0, 1.0, 0.0)
                      + MIN(MAX(?4 - CAST(s.ts AS INTEGER), 0) / 86400.0, 365.0) * 0.01
                      AS score
               FROM search_index s
               LEFT JOIN messages m
                 ON s.entity_type = 'mail' AND m.id = s.entity_id
              WHERE search_index MATCH ?1{entity_guard}{preds}
              ORDER BY score
              LIMIT ?"
        );
        let mut all_params: Vec<Value> = vec![
            Value::Text(match_expr),
            Value::Text(HIGHLIGHT_START.to_string()),
            Value::Text(HIGHLIGHT_END.to_string()),
            Value::Integer(now),
        ];
        all_params.extend(pred_params);
        all_params.push(Value::Integer(i64::from(limit)));

        let mut stmt = conn.prepare(&sql)?;
        let hits = stmt
            .query_map(rusqlite::params_from_iter(all_params), |row| {
                let entity_type: String = row.get(0)?;
                let summary = if entity_type == "mail" {
                    let id: Option<String> = row.get(5)?;
                    id.map(|id| {
                        Ok::<_, rusqlite::Error>(MessageSummary {
                            id,
                            folder_id: row.get(6)?,
                            subject: row.get(7)?,
                            from_name: row.get(8)?,
                            from_address: row.get(9)?,
                            received_at: row.get(10)?,
                            preview: row.get(11)?,
                            is_read: row.get(12)?,
                            has_attachments: row.get(13)?,
                            flag_status: row.get(14)?,
                            inference_classification: row.get(15)?,
                            is_draft: row.get(16)?,
                        })
                    })
                    .transpose()?
                } else {
                    None
                };
                Ok(SearchHit {
                    entity_type,
                    entity_id: row.get(1)?,
                    title: row.get(2)?,
                    snippet: row.get(3)?,
                    ts: row.get::<_, Option<i64>>(4)?.unwrap_or(0),
                    summary,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(hits)
    }

    fn index_document(
        &self,
        entity_type: &str,
        entity_id: &str,
        title: &str,
        subtitle: &str,
        body: &str,
        ts: i64,
    ) -> Result<(), MailStoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM search_index WHERE entity_type = ?1 AND entity_id = ?2",
            params![entity_type, entity_id],
        )?;
        conn.execute(
            "INSERT INTO search_index (entity_type, entity_id, title, subtitle, body, ts)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![entity_type, entity_id, title, subtitle, body, ts],
        )?;
        Ok(())
    }

    fn list_saved_searches(&self) -> Result<Vec<SavedSearch>, MailStoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT id, name, query, filters FROM saved_searches ORDER BY name")?;
        let saved = stmt
            .query_map([], |row| {
                Ok(SavedSearch {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    query: row.get(2)?,
                    filters: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(saved)
    }

    fn save_search(
        &self,
        name: &str,
        query: &str,
        filters_json: &str,
    ) -> Result<i64, MailStoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO saved_searches (name, query, filters, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![name, query, filters_json, crate::sync::now_epoch()],
        )?;
        Ok(conn.last_insert_rowid())
    }

    fn delete_saved_search(&self, id: i64) -> Result<(), MailStoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM saved_searches WHERE id = ?1", params![id])?;
        Ok(())
    }

    fn message_exists(
        &self,
        id: &str,
        internet_message_id: Option<&str>,
    ) -> Result<bool, MailStoreError> {
        let conn = self.conn.lock().unwrap();
        let exists: bool = conn.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM messages
                  WHERE id = ?1
                     OR (?2 IS NOT NULL AND internet_message_id = ?2))",
            params![id, internet_message_id],
            |row| row.get(0),
        )?;
        Ok(exists)
    }

    fn broken_summary_ids(&self, limit: u32) -> Result<Vec<String>, MailStoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id FROM messages WHERE from_address = '' AND received_at = 0 LIMIT ?1",
        )?;
        let ids = stmt
            .query_map(params![limit], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(ids)
    }

    fn set_read_state(&self, id: &str, is_read: bool) -> Result<(), MailStoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE messages SET is_read = ?2 WHERE id = ?1",
            params![id, is_read],
        )?;
        Ok(())
    }

    fn set_flag_status(&self, id: &str, flag_status: &str) -> Result<(), MailStoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE messages SET flag_status = ?2 WHERE id = ?1",
            params![id, flag_status],
        )?;
        Ok(())
    }

    fn set_categories(&self, id: &str, categories: &[String]) -> Result<(), MailStoreError> {
        let json = serde_json::to_string(categories).unwrap_or_else(|_| "[]".to_string());
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE messages SET categories = ?2 WHERE id = ?1",
            params![id, json],
        )?;
        Ok(())
    }

    fn set_inference(&self, id: &str, classification: &str) -> Result<(), MailStoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE messages SET inference_classification = ?2 WHERE id = ?1",
            params![id, classification],
        )?;
        Ok(())
    }

    fn move_message(
        &self,
        id: &str,
        new_folder_id: &str,
        new_id: Option<&str>,
    ) -> Result<(), MailStoreError> {
        let conn = self.conn.lock().unwrap();
        match new_id {
            // Confirmed Graph move: the destination copy has a new id.
            Some(dest) if dest != id => {
                conn.execute(
                    "UPDATE OR REPLACE messages SET id = ?2, folder_id = ?3 WHERE id = ?1",
                    params![id, dest, new_folder_id],
                )?;
            }
            _ => {
                conn.execute(
                    "UPDATE messages SET folder_id = ?2 WHERE id = ?1",
                    params![id, new_folder_id],
                )?;
            }
        }
        Ok(())
    }
}

/// A non-mail search document in the memory backend:
/// (entity_type, entity_id, title, subtitle, body, ts).
type MemoryDoc = (String, String, String, String, String, i64);

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
    /// Non-mail documents added via index_document.
    documents: Mutex<Vec<MemoryDoc>>,
    saved_searches: Mutex<Vec<SavedSearch>>,
    saved_seq: std::sync::atomic::AtomicI64,
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
                if m.internet_message_id.is_none() {
                    m.internet_message_id = existing.internet_message_id.clone();
                }
                if m.conversation_id.is_none() {
                    m.conversation_id = existing.conversation_id.clone();
                }
                // Mirror the SQLite guard: a partial delta change-event (empty
                // sender + no received date) must not blank a good summary.
                let partial = m.summary.from_address.is_empty() && m.summary.received_at == 0;
                if partial {
                    m.summary.subject = existing.summary.subject.clone();
                    m.summary.from_name = existing.summary.from_name.clone();
                    m.summary.from_address = existing.summary.from_address.clone();
                    m.summary.preview = existing.summary.preview.clone();
                    m.summary.has_attachments = existing.summary.has_attachments;
                    m.categories = existing.categories.clone();
                }
                if m.summary.received_at == 0 {
                    m.summary.received_at = existing.summary.received_at;
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
            if !is_smtp_address(address) {
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
            if is_smtp_address(&addr) && matches(&addr, &m.summary.from_name) {
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

    fn search(
        &self,
        query: &str,
        filters: &SearchFilters,
        limit: u32,
    ) -> Result<Vec<SearchHit>, MailStoreError> {
        let tokens: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        let matches = |haystack: &str| {
            let h = haystack.to_lowercase();
            tokens.iter().all(|t| h.contains(t))
        };
        let passes = |s: &MessageSummary| {
            filters.folder_id.as_ref().is_none_or(|f| &s.folder_id == f)
                && (!filters.unread_only || !s.is_read)
                && (!filters.has_attachments || s.has_attachments)
                && (!filters.flagged_only || s.flag_status == "flagged")
                && filters.from.as_deref().is_none_or(|f| {
                    let f = f.to_lowercase();
                    s.from_address.to_lowercase().contains(&f)
                        || s.from_name.to_lowercase().contains(&f)
                })
                && filters.date_from.is_none_or(|d| s.received_at >= d)
                && filters.date_to.is_none_or(|d| s.received_at <= d)
        };

        let mut hits: Vec<SearchHit> = Vec::new();
        for m in self.messages.lock().unwrap().values() {
            let s = &m.summary;
            let text = format!(
                "{} {} {} {} {}",
                s.subject,
                s.from_name,
                s.from_address,
                s.preview,
                m.body_html.as_deref().map(html_to_text).unwrap_or_default()
            );
            if passes(s) && (tokens.is_empty() || matches(&text)) {
                hits.push(SearchHit {
                    entity_type: "mail".to_string(),
                    entity_id: s.id.clone(),
                    title: s.subject.clone(),
                    snippet: s.preview.clone(),
                    ts: s.received_at,
                    summary: Some(s.clone()),
                });
            }
        }
        if filters.is_empty() && !tokens.is_empty() {
            for (etype, eid, title, subtitle, body, ts) in self.documents.lock().unwrap().iter() {
                if matches(&format!("{title} {subtitle} {body}")) {
                    hits.push(SearchHit {
                        entity_type: etype.clone(),
                        entity_id: eid.clone(),
                        title: title.clone(),
                        snippet: body.clone(),
                        ts: *ts,
                        summary: None,
                    });
                }
            }
        }
        hits.sort_by_key(|h| std::cmp::Reverse(h.ts));
        hits.truncate(limit as usize);
        Ok(hits)
    }

    fn index_document(
        &self,
        entity_type: &str,
        entity_id: &str,
        title: &str,
        subtitle: &str,
        body: &str,
        ts: i64,
    ) -> Result<(), MailStoreError> {
        let mut docs = self.documents.lock().unwrap();
        docs.retain(|(t, i, ..)| !(t == entity_type && i == entity_id));
        docs.push((
            entity_type.to_string(),
            entity_id.to_string(),
            title.to_string(),
            subtitle.to_string(),
            body.to_string(),
            ts,
        ));
        Ok(())
    }

    fn list_saved_searches(&self) -> Result<Vec<SavedSearch>, MailStoreError> {
        let mut saved = self.saved_searches.lock().unwrap().clone();
        saved.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(saved)
    }

    fn save_search(
        &self,
        name: &str,
        query: &str,
        filters_json: &str,
    ) -> Result<i64, MailStoreError> {
        let id = self
            .saved_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        self.saved_searches.lock().unwrap().push(SavedSearch {
            id,
            name: name.to_string(),
            query: query.to_string(),
            filters: filters_json.to_string(),
        });
        Ok(id)
    }

    fn delete_saved_search(&self, id: i64) -> Result<(), MailStoreError> {
        self.saved_searches.lock().unwrap().retain(|s| s.id != id);
        Ok(())
    }

    fn message_exists(
        &self,
        id: &str,
        internet_message_id: Option<&str>,
    ) -> Result<bool, MailStoreError> {
        let map = self.messages.lock().unwrap();
        Ok(map.contains_key(id)
            || internet_message_id.is_some_and(|imid| {
                map.values()
                    .any(|m| m.internet_message_id.as_deref() == Some(imid))
            }))
    }

    fn broken_summary_ids(&self, limit: u32) -> Result<Vec<String>, MailStoreError> {
        Ok(self
            .messages
            .lock()
            .unwrap()
            .values()
            .filter(|m| m.summary.from_address.is_empty() && m.summary.received_at == 0)
            .map(|m| m.summary.id.clone())
            .take(limit as usize)
            .collect())
    }

    fn set_read_state(&self, id: &str, is_read: bool) -> Result<(), MailStoreError> {
        if let Some(m) = self.messages.lock().unwrap().get_mut(id) {
            m.summary.is_read = is_read;
        }
        Ok(())
    }

    fn set_flag_status(&self, id: &str, flag_status: &str) -> Result<(), MailStoreError> {
        if let Some(m) = self.messages.lock().unwrap().get_mut(id) {
            m.summary.flag_status = flag_status.to_string();
        }
        Ok(())
    }

    fn set_categories(&self, id: &str, categories: &[String]) -> Result<(), MailStoreError> {
        if let Some(m) = self.messages.lock().unwrap().get_mut(id) {
            m.categories = categories.to_vec();
        }
        Ok(())
    }

    fn set_inference(&self, id: &str, classification: &str) -> Result<(), MailStoreError> {
        if let Some(m) = self.messages.lock().unwrap().get_mut(id) {
            m.summary.inference_classification = classification.to_string();
        }
        Ok(())
    }

    fn move_message(
        &self,
        id: &str,
        new_folder_id: &str,
        new_id: Option<&str>,
    ) -> Result<(), MailStoreError> {
        let mut map = self.messages.lock().unwrap();
        if let Some(mut m) = map.remove(id) {
            m.summary.folder_id = new_folder_id.to_string();
            let key = match new_id {
                Some(dest) => {
                    m.summary.id = dest.to_string();
                    dest.to_string()
                }
                None => id.to_string(),
            };
            map.insert(key, m);
        }
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
                flag_status: "notFlagged".to_string(),
                inference_classification: "focused".to_string(),
                is_draft: false,
            },
            conversation_id: None,
            body_html: body.map(str::to_string),
            body_content_type: body.map(|_| "html".to_string()),
            internet_message_id: Some(format!("{id}@mail.example")),
            categories: Vec::new(),
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

        // Exchange legacy X.500 DNs and other non-SMTP shapes are never
        // cached and never suggested — from either source branch.
        store
            .record_recipients(
                &[
                    (
                        "/o=First Organization/ou=Exchange Administrative \
                         Group/cn=Recipients/cn=abc123"
                            .into(),
                        "Legacy Dn".into(),
                    ),
                    ("not-an-address".into(), "No At".into()),
                ],
                300,
            )
            .unwrap();
        assert!(store.search_recipients("Legacy", 5).unwrap().is_empty());
        assert!(store.search_recipients("No At", 5).unwrap().is_empty());
        let mut dn_msg = message("m-dn", "f1", 400, None);
        dn_msg.summary.from_address = "/o=Org/ou=X/cn=Recipients/cn=dnsender".into();
        dn_msg.summary.from_name = "Dn Sender".into();
        store.upsert_messages(&[dn_msg]).unwrap();
        assert!(
            store.search_recipients("Dn Sender", 5).unwrap().is_empty(),
            "message-derived DN sender must not surface"
        );

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

        // Saved searches: save, list (sorted), delete — round-trips on both
        // backends.
        let id_a = store
            .save_search("Unread from Alice", "alice", r#"{"unread_only":true}"#)
            .unwrap();
        store
            .save_search("Attachments", "", r#"{"has_attachments":true}"#)
            .unwrap();
        let saved = store.list_saved_searches().unwrap();
        assert_eq!(saved.len(), 2);
        assert_eq!(saved[0].name, "Attachments");
        assert_eq!(saved[1].query, "alice");
        store.delete_saved_search(id_a).unwrap();
        assert_eq!(store.list_saved_searches().unwrap().len(), 1);

        // Server-hit dedupe: known by Graph id, known by internetMessageId,
        // unknown otherwise.
        assert!(store.message_exists("m1", None).unwrap());
        assert!(store
            .message_exists("some-other-graph-id", Some("m1@mail.example"))
            .unwrap());
        assert!(!store
            .message_exists("nope", Some("nope@mail.example"))
            .unwrap());

        // Basic search smoke on the shared contract (backend-specific FTS
        // behavior is covered in the sqlite-only tests).
        let hits = store
            .search("subject m4", &SearchFilters::default(), 10)
            .unwrap();
        assert!(hits.iter().any(|h| h.entity_id == "m4"));

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

    /// A partial delta change-event (empty sender + received_at 0, as Graph
    /// sends for a bare read/flag flip) must NOT blank a good summary — only
    /// the state flags update. Regression for the "(unknown sender)" bug.
    fn partial_event_preserves_summary(store: &dyn MailStore) {
        store.upsert_folders(&[folder("f1", "Inbox")]).unwrap();
        let mut full = message("m1", "f1", 1000, Some("<p>body</p>"));
        full.summary.subject = "Real subject".into();
        full.summary.from_name = "Alice".into();
        full.summary.from_address = "alice@example.com".into();
        full.summary.preview = "hello there".into();
        full.summary.has_attachments = true;
        store.upsert_messages(&[full]).unwrap();

        // A bare change-event: same id, is_read flipped, everything else empty.
        let partial = Message {
            summary: MessageSummary {
                id: "m1".into(),
                folder_id: "f1".into(),
                subject: String::new(),
                from_name: String::new(),
                from_address: String::new(),
                received_at: 0,
                preview: String::new(),
                is_read: true,
                has_attachments: false,
                flag_status: "flagged".into(),
                inference_classification: "focused".into(),
                is_draft: false,
            },
            conversation_id: None,
            body_html: None,
            body_content_type: None,
            internet_message_id: None,
            categories: vec![],
        };
        store.upsert_messages(&[partial]).unwrap();

        let m = store.get_message("m1").unwrap().unwrap();
        // Descriptive fields preserved.
        assert_eq!(m.summary.subject, "Real subject");
        assert_eq!(m.summary.from_address, "alice@example.com");
        assert_eq!(m.summary.preview, "hello there");
        assert_eq!(m.summary.received_at, 1000);
        assert!(m.summary.has_attachments);
        assert!(m.body_html.is_some(), "cached body preserved");
        // State the event actually carried is applied.
        assert!(m.summary.is_read);
        assert_eq!(m.summary.flag_status, "flagged");

        // A *full* update (real sender + date) still overwrites normally.
        let mut updated = message("m1", "f1", 2000, None);
        updated.summary.subject = "Edited".into();
        updated.summary.from_address = "bob@example.com".into();
        store.upsert_messages(&[updated]).unwrap();
        let m = store.get_message("m1").unwrap().unwrap();
        assert_eq!(m.summary.subject, "Edited");
        assert_eq!(m.summary.from_address, "bob@example.com");

        // broken_summary_ids finds a genuinely-blank row and not a good one.
        let blank = Message {
            summary: MessageSummary {
                id: "blank".into(),
                folder_id: "f1".into(),
                subject: String::new(),
                from_name: String::new(),
                from_address: String::new(),
                received_at: 0,
                preview: String::new(),
                is_read: false,
                has_attachments: false,
                flag_status: "notFlagged".into(),
                inference_classification: "focused".into(),
                is_draft: false,
            },
            conversation_id: None,
            body_html: None,
            body_content_type: None,
            internet_message_id: None,
            categories: vec![],
        };
        store.upsert_messages(&[blank]).unwrap();
        let broken = store.broken_summary_ids(10).unwrap();
        assert_eq!(broken, ["blank"]);
    }

    #[test]
    fn partial_event_guard_memory() {
        partial_event_preserves_summary(&MemoryMailStore::default());
    }

    #[test]
    fn partial_event_guard_sqlite() {
        partial_event_preserves_summary(&SqliteMailStore::open_in_memory().unwrap());
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
        assert_eq!(version, 6);
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
            "saved_searches",
            "search_index",
        ] {
            assert!(tables.iter().any(|t| t == required), "missing {required}");
        }
    }

    // NOTE: rusqlite_migration's validate() can't run here — the v5 backfill
    // calls the strip_html() SQL function, which only exists on connections
    // opened through SqliteMailStore::init. The open()-based tests below
    // exercise the full migration chain instead.

    #[test]
    fn smtp_address_validation() {
        for ok in [
            "user@example.com",
            "first.last@sub.example.co.uk",
            "UPPER@Example.COM",
        ] {
            assert!(is_smtp_address(ok), "{ok} should be valid");
        }
        for bad in [
            "/o=First Organization/ou=Exchange Administrative Group/cn=Recipients/cn=abc",
            "no-at-sign",
            "@example.com",
            "user@",
            "user@localhost",
            "user@.com",
            "user@example.",
            "a@b@c.com",
            "",
        ] {
            assert!(!is_smtp_address(bad), "{bad} should be rejected");
        }
    }

    fn seeded_store() -> SqliteMailStore {
        let store = SqliteMailStore::open_in_memory().unwrap();
        store
            .upsert_folders(&[folder("f1", "Inbox"), folder("f2", "Archive")])
            .unwrap();
        let mut quarterly = message("m1", "f1", 1_000, Some("<p>revenue tables attached</p>"));
        quarterly.summary.subject = "Quarterly numbers".into();
        quarterly.summary.from_name = "Alice Café".into();
        quarterly.summary.from_address = "alice@example.com".into();
        let mut lunch = message("m2", "f1", 2_000, None);
        lunch.summary.subject = "Lunch tomorrow?".into();
        lunch.summary.from_name = "Bob".into();
        lunch.summary.from_address = "bob@example.com".into();
        lunch.summary.preview = "new cafeteria menu".into();
        lunch.summary.has_attachments = true;
        let mut old = message("m3", "f2", 500, None);
        old.summary.subject = "Archived quarterly plan".into();
        old.summary.is_read = true;
        store.upsert_messages(&[quarterly, lunch, old]).unwrap();
        store
    }

    #[test]
    fn fts_query_builder_neutralizes_operators() {
        assert_eq!(fts_query("hello world"), Some(r#""hello" "world"*"#.into()));
        assert_eq!(fts_query("  "), None);
        assert_eq!(fts_query("a:b"), Some(r#""a:b"*"#.into()));
        assert_eq!(fts_query(r#"say "hi""#), Some(r#""say" """hi"""*"#.into()));
        // Operator words are quoted into plain tokens.
        assert_eq!(fts_query("AND"), Some(r#""AND"*"#.into()));
    }

    #[test]
    fn fts_search_ranks_highlights_and_prefixes() {
        let store = seeded_store();
        // Prefix match while typing.
        let hits = store.search("quar", &SearchFilters::default(), 10).unwrap();
        assert_eq!(hits.len(), 2, "{hits:?}");
        assert!(hits.iter().all(|h| h.entity_type == "mail"));
        // Title highlighting uses the private-use markers.
        assert!(
            hits[0].title.contains(HIGHLIGHT_START) && hits[0].title.contains(HIGHLIGHT_END),
            "{:?}",
            hits[0].title
        );
        // Diacritic-insensitive: 'cafe' finds both 'Café' (subtitle) and
        // 'cafeteria' (prefix in body/preview).
        let hits = store.search("cafe", &SearchFilters::default(), 10).unwrap();
        assert!(!hits.is_empty());
        // Body text (stripped from HTML) is searchable; no tag tokens.
        let hits = store
            .search("revenue", &SearchFilters::default(), 10)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].entity_id, "m1");
        assert!(store
            .search("div", &SearchFilters::default(), 10)
            .unwrap()
            .is_empty());
        // Malicious-looking query input cannot break the MATCH expression.
        for weird in [r#"a" OR "b"#, "col:umn", "NEAR(", "(((", "\"\"\""] {
            store.search(weird, &SearchFilters::default(), 10).unwrap();
        }
    }

    #[test]
    fn fts_index_follows_message_lifecycle() {
        let store = seeded_store();
        // Body cache upgrade reindexes: 'zeppelin' only exists in the body
        // set after the initial insert.
        assert!(store
            .search("zeppelin", &SearchFilters::default(), 10)
            .unwrap()
            .is_empty());
        store
            .set_message_body("m2", "<p>the zeppelin schedule</p>", "html")
            .unwrap();
        let hits = store
            .search("zeppelin", &SearchFilters::default(), 10)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].entity_id, "m2");
        // Deletion removes the index row — no orphans.
        store.delete_message("m2").unwrap();
        assert!(store
            .search("zeppelin", &SearchFilters::default(), 10)
            .unwrap()
            .is_empty());
        // Summary-only re-upsert (delta replay) keeps the row searchable.
        store
            .upsert_messages(&[message("m1", "f1", 1_000, None)])
            .unwrap();
        assert!(!store
            .search("subject", &SearchFilters::default(), 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn search_filters_and_browse() {
        let store = seeded_store();
        let mut f = SearchFilters {
            folder_id: Some("f1".into()),
            ..Default::default()
        };
        let hits = store.search("quarterly", &f, 10).unwrap();
        assert_eq!(hits.len(), 1, "folder scope narrows to the inbox copy");
        assert_eq!(hits[0].entity_id, "m1");

        f = SearchFilters {
            has_attachments: true,
            ..Default::default()
        };
        let hits = store.search("", &f, 10).unwrap();
        assert_eq!(hits.len(), 1, "filtered browse with empty query");
        assert_eq!(hits[0].entity_id, "m2");

        f = SearchFilters {
            unread_only: true,
            from: Some("alice".into()),
            ..Default::default()
        };
        let hits = store.search("", &f, 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].entity_id, "m1");

        f = SearchFilters {
            date_from: Some(900),
            date_to: Some(1_500),
            ..Default::default()
        };
        let hits = store.search("", &f, 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].entity_id, "m1");
    }

    #[test]
    fn search_index_is_entity_agnostic() {
        let store = seeded_store();
        store
            .index_document(
                "event",
                "ev-1",
                "Design review",
                "Conference room 4",
                "walk through the quarterly mockups",
                3_000,
            )
            .unwrap();
        let hits = store
            .search("design review", &SearchFilters::default(), 10)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].entity_type, "event");
        assert_eq!(hits[0].entity_id, "ev-1");
        assert!(hits[0].summary.is_none());
        // Events and mail rank together in one query.
        let hits = store
            .search("quarterly", &SearchFilters::default(), 10)
            .unwrap();
        assert!(hits.iter().any(|h| h.entity_type == "event"));
        assert!(hits.iter().any(|h| h.entity_type == "mail"));
        // Mail filters exclude non-mail entities.
        let f = SearchFilters {
            folder_id: Some("f1".into()),
            ..Default::default()
        };
        let hits = store.search("quarterly", &f, 10).unwrap();
        assert!(hits.iter().all(|h| h.entity_type == "mail"));
        // Re-indexing the same document replaces, not duplicates.
        store
            .index_document("event", "ev-1", "Design review v2", "", "updated", 3_100)
            .unwrap();
        let hits = store
            .search("design", &SearchFilters::default(), 10)
            .unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn write_mutators_and_flagged_filter() {
        for store in [
            Box::new(seeded_store()) as Box<dyn MailStore>,
            Box::new({
                let s = MemoryMailStore::default();
                s.upsert_folders(&[folder("f1", "Inbox")]).unwrap();
                s.upsert_messages(&[message("m1", "f1", 1_000, None)])
                    .unwrap();
                s
            }),
        ] {
            // Read state.
            store.set_read_state("m1", true).unwrap();
            assert!(store.get_message("m1").unwrap().unwrap().summary.is_read);
            store.set_read_state("m1", false).unwrap();
            assert!(!store.get_message("m1").unwrap().unwrap().summary.is_read);

            // Flag status + the flagged filter (Phase 6's disabled chip).
            store.set_flag_status("m1", "flagged").unwrap();
            let f = SearchFilters {
                flagged_only: true,
                ..Default::default()
            };
            let hits = store.search("", &f, 10).unwrap();
            assert!(hits.iter().any(|h| h.entity_id == "m1"));
            store.set_flag_status("m1", "notFlagged").unwrap();
            assert!(store.search("", &f, 10).unwrap().is_empty());

            // Categories.
            store
                .set_categories("m1", &["Red".to_string(), "Work".to_string()])
                .unwrap();
            assert_eq!(
                store.get_message("m1").unwrap().unwrap().categories,
                vec!["Red".to_string(), "Work".to_string()]
            );

            // Inference (focused/other).
            store.set_inference("m1", "other").unwrap();
            assert_eq!(
                store
                    .get_message("m1")
                    .unwrap()
                    .unwrap()
                    .summary
                    .inference_classification,
                "other"
            );

            // Move with id reconciliation: old id gone, new id in new folder.
            store
                .upsert_folders(&[folder("archive", "Archive")])
                .unwrap();
            store
                .move_message("m1", "archive", Some("m1-moved"))
                .unwrap();
            assert!(store.get_message("m1").unwrap().is_none());
            let moved = store.get_message("m1-moved").unwrap().unwrap();
            assert_eq!(moved.summary.folder_id, "archive");
            // Plain move (no new id) just repoints the folder.
            store.move_message("m1-moved", "f1", None).unwrap();
            assert_eq!(
                store
                    .get_message("m1-moved")
                    .unwrap()
                    .unwrap()
                    .summary
                    .folder_id,
                "f1"
            );
        }
    }

    #[test]
    fn kql_builder_composes_query_and_filters() {
        let f = SearchFilters {
            from: Some("alice@example.com".into()),
            has_attachments: true,
            date_from: Some(1_751_328_000), // 2025-07-01
            ..Default::default()
        };
        let kql = build_kql(r#"quarterly "numbers""#, &f);
        assert_eq!(
            kql,
            "quarterly numbers from:alice@example.com hasAttachment:true received>=2025-07-01"
        );
        assert_eq!(build_kql("", &SearchFilters::default()), "");
    }

    #[test]
    fn migration_v4_purges_legacy_dn_contacts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("spite.db");
        // Build a v3 database by hand with a legacy row already present.
        {
            let mut conn = Connection::open(&path).unwrap();
            let v3 = Migrations::new(migration_steps().into_iter().take(3).collect());
            v3.to_latest(&mut conn).unwrap();
            conn.execute(
                "INSERT INTO contacts (address, name, uses, last_used) VALUES
                 ('/o=org/ou=x/cn=recipients/cn=legacy', 'Legacy', 5, 1),
                 ('keep@example.com', 'Keep', 3, 1),
                 ('bare-string', '', 1, 1)",
                [],
            )
            .unwrap();
        }
        // Opening through the store runs the v4 step.
        let store = SqliteMailStore::open(&path).unwrap();
        let hits = store.search_recipients("e", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, "keep@example.com");
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
