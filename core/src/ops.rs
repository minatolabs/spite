//! Optimistic mail-management writes with rollback + reconciliation.
//!
//! Every write follows one pipeline (`execute_op`): apply the change to the
//! local store *first* so the UI feels instant, fire the Graph call, and on
//! failure roll the local state back to exactly what it was — the UI never
//! keeps a state the server rejected. Delta sync is the backstop reconciler.
//!
//! `MailWriter` is the Graph write surface behind a trait, mirroring
//! `MailSource`, so the whole pipeline is unit-tested against a mock with no
//! network.

use std::future::Future;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::store::{MailStore, MailStoreError, Message};
use crate::sync::SourceError;

#[derive(Debug, thiserror::Error)]
pub enum OpError {
    #[error(transparent)]
    Source(#[from] SourceError),
    #[error(transparent)]
    Store(#[from] MailStoreError),
    #[error("message not found: {0}")]
    NotFound(String),
    #[error("background task failed: {0}")]
    Join(String),
}

/// A single mail-management operation. Carries only its inputs; the previous
/// state needed for rollback is captured from the store at execution time.
// `rename_all` renames the variant names (SetRead → setRead, matching the
// `kind` tag from JS); `rename_all_fields` is REQUIRED to also camelCase the
// struct-variant fields (is_read → isRead, dest_folder_id → destFolderId).
// Without the latter, ops carrying multi-word fields fail to deserialize
// before ever reaching Graph — the bug that made mark-read/move/archive/
// delete silently do nothing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum MailOp {
    SetRead {
        id: String,
        is_read: bool,
    },
    SetFlag {
        id: String,
        flagged: bool,
    },
    SetCategories {
        id: String,
        categories: Vec<String>,
    },
    SetInference {
        id: String,
        focused: bool,
    },
    /// Move to a folder (archive/delete-to-trash/user move all funnel here).
    Move {
        id: String,
        dest_folder_id: String,
    },
    /// Permanent delete — no rollback of content is possible if it succeeds,
    /// but a *failed* delete restores the row.
    HardDelete {
        id: String,
    },
}

impl MailOp {
    pub fn message_id(&self) -> &str {
        match self {
            MailOp::SetRead { id, .. }
            | MailOp::SetFlag { id, .. }
            | MailOp::SetCategories { id, .. }
            | MailOp::SetInference { id, .. }
            | MailOp::Move { id, .. }
            | MailOp::HardDelete { id } => id,
        }
    }
}

/// The Graph write surface. Implemented by `graph::GraphMailSource`, mocked
/// in tests. Move returns the destination copy's new message id (Graph
/// assigns a fresh id on move).
pub trait MailWriter: Send + Sync {
    fn set_read(
        &self,
        id: &str,
        is_read: bool,
    ) -> impl Future<Output = Result<(), SourceError>> + Send;
    fn set_flag(
        &self,
        id: &str,
        flagged: bool,
    ) -> impl Future<Output = Result<(), SourceError>> + Send;
    fn set_categories(
        &self,
        id: &str,
        categories: &[String],
    ) -> impl Future<Output = Result<(), SourceError>> + Send;
    fn set_inference(
        &self,
        id: &str,
        focused: bool,
    ) -> impl Future<Output = Result<(), SourceError>> + Send;
    /// Returns the moved message's new id in the destination folder.
    fn move_message(
        &self,
        id: &str,
        dest_folder_id: &str,
    ) -> impl Future<Output = Result<String, SourceError>> + Send;
    fn delete_message(&self, id: &str) -> impl Future<Output = Result<(), SourceError>> + Send;
}

/// Snapshot of a message's pre-op state, for rollback.
enum Rollback {
    Read {
        id: String,
        is_read: bool,
    },
    Flag {
        id: String,
        flag_status: String,
    },
    Categories {
        id: String,
        categories: Vec<String>,
    },
    Inference {
        id: String,
        classification: String,
    },
    /// Restore a moved/deleted message wholesale (folder change or removal).
    RestoreMessage(Box<Message>),
}

async fn store_call<T, F>(store: &Arc<dyn MailStore>, f: F) -> Result<T, OpError>
where
    F: FnOnce(&dyn MailStore) -> Result<T, MailStoreError> + Send + 'static,
    T: Send + 'static,
{
    let store = Arc::clone(store);
    tokio::task::spawn_blocking(move || f(store.as_ref()))
        .await
        .map_err(|e| OpError::Join(e.to_string()))?
        .map_err(OpError::from)
}

/// Apply `op` optimistically, call Graph, and roll back on failure. Returns
/// the (possibly new) message id — `Move` changes it, everything else keeps
/// it; `HardDelete` returns the deleted id.
pub async fn execute_op(
    store: &Arc<dyn MailStore>,
    writer: &impl MailWriter,
    op: MailOp,
) -> Result<String, OpError> {
    let id = op.message_id().to_string();
    let current = {
        let id = id.clone();
        store_call(store, move |s| s.get_message(&id)).await?
    }
    .ok_or_else(|| OpError::NotFound(id.clone()))?;

    // 1. Apply locally + capture rollback state.
    let rollback = apply_local(store, &op, &current).await?;

    // 2. Fire the Graph call.
    let result = call_graph(writer, &op).await;

    // 3. Reconcile.
    match result {
        Ok(new_id) => {
            if let (MailOp::Move { dest_folder_id, .. }, Some(new_id)) = (&op, &new_id) {
                // Graph move minted a new id in the destination; swap the
                // optimistic local row over to it (FTS triggers follow).
                let (id, dest, new_id) = (id.clone(), dest_folder_id.clone(), new_id.clone());
                store_call(store, move |s| s.move_message(&id, &dest, Some(&new_id))).await?;
            }
            Ok(new_id.unwrap_or(id))
        }
        Err(e) => {
            // Roll the optimistic change back to exactly the prior state.
            rollback_local(store, rollback).await?;
            Err(e.into())
        }
    }
}

async fn apply_local(
    store: &Arc<dyn MailStore>,
    op: &MailOp,
    current: &Message,
) -> Result<Rollback, OpError> {
    let s = &current.summary;
    Ok(match op {
        MailOp::SetRead { id, is_read } => {
            let rb = Rollback::Read {
                id: id.clone(),
                is_read: s.is_read,
            };
            let (id, v) = (id.clone(), *is_read);
            store_call(store, move |st| st.set_read_state(&id, v)).await?;
            rb
        }
        MailOp::SetFlag { id, flagged } => {
            let rb = Rollback::Flag {
                id: id.clone(),
                flag_status: s.flag_status.clone(),
            };
            let (id, status) = (
                id.clone(),
                if *flagged { "flagged" } else { "notFlagged" }.to_string(),
            );
            store_call(store, move |st| st.set_flag_status(&id, &status)).await?;
            rb
        }
        MailOp::SetCategories { id, categories } => {
            let rb = Rollback::Categories {
                id: id.clone(),
                categories: current.categories.clone(),
            };
            let (id, cats) = (id.clone(), categories.clone());
            store_call(store, move |st| st.set_categories(&id, &cats)).await?;
            rb
        }
        MailOp::SetInference { id, focused } => {
            let rb = Rollback::Inference {
                id: id.clone(),
                classification: s.inference_classification.clone(),
            };
            let (id, c) = (
                id.clone(),
                if *focused { "focused" } else { "other" }.to_string(),
            );
            store_call(store, move |st| st.set_inference(&id, &c)).await?;
            rb
        }
        MailOp::Move { id, dest_folder_id } => {
            let rb = Rollback::RestoreMessage(Box::new(current.clone()));
            let (id, dest) = (id.clone(), dest_folder_id.clone());
            store_call(store, move |st| st.move_message(&id, &dest, None)).await?;
            rb
        }
        MailOp::HardDelete { id } => {
            let rb = Rollback::RestoreMessage(Box::new(current.clone()));
            let id = id.clone();
            store_call(store, move |st| st.delete_message(&id)).await?;
            rb
        }
    })
}

async fn call_graph(writer: &impl MailWriter, op: &MailOp) -> Result<Option<String>, SourceError> {
    match op {
        MailOp::SetRead { id, is_read } => writer.set_read(id, *is_read).await.map(|_| None),
        MailOp::SetFlag { id, flagged } => writer.set_flag(id, *flagged).await.map(|_| None),
        MailOp::SetCategories { id, categories } => {
            writer.set_categories(id, categories).await.map(|_| None)
        }
        MailOp::SetInference { id, focused } => {
            writer.set_inference(id, *focused).await.map(|_| None)
        }
        MailOp::Move { id, dest_folder_id } => {
            writer.move_message(id, dest_folder_id).await.map(Some)
        }
        MailOp::HardDelete { id } => writer.delete_message(id).await.map(|_| None),
    }
}

async fn rollback_local(store: &Arc<dyn MailStore>, rb: Rollback) -> Result<(), OpError> {
    match rb {
        Rollback::Read { id, is_read } => {
            store_call(store, move |s| s.set_read_state(&id, is_read)).await?
        }
        Rollback::Flag { id, flag_status } => {
            store_call(store, move |s| s.set_flag_status(&id, &flag_status)).await?
        }
        Rollback::Categories { id, categories } => {
            store_call(store, move |s| s.set_categories(&id, &categories)).await?
        }
        Rollback::Inference { id, classification } => {
            store_call(store, move |s| s.set_inference(&id, &classification)).await?
        }
        // Re-upsert restores a moved row to its old folder or resurrects a
        // deleted one, byte-for-byte from the pre-op snapshot.
        Rollback::RestoreMessage(m) => store_call(store, move |s| s.upsert_messages(&[*m])).await?,
    }
    Ok(())
}

// ============================================================================
// Bulk operations via Microsoft Graph JSON batching (POST /$batch).
// ============================================================================

/// One sub-request inside a `$batch`. `index` is its position in the input
/// op list and doubles as the batch correlation `id`.
#[derive(Debug, Clone)]
pub struct BatchSub {
    pub index: usize,
    pub method: &'static str,
    /// Relative Graph URL, e.g. `/me/messages/{id}/move`.
    pub url: String,
    pub body: Option<serde_json::Value>,
}

/// The per-sub-request result the writer hands back, correlated by `index`.
#[derive(Debug, Clone)]
pub struct SubResult {
    pub index: usize,
    pub status: u16,
    pub body: serde_json::Value,
}

/// The Graph `$batch` surface. One call sends ONE chunk (≤20). Implemented by
/// `graph::GraphMailSource`; mocked in tests. A transport-level failure is an
/// `Err`; per-item HTTP failures come back as non-2xx `SubResult`s.
pub trait MailBatchWriter: Send + Sync {
    fn execute_chunk(
        &self,
        subs: &[BatchSub],
    ) -> impl Future<Output = Result<Vec<SubResult>, SourceError>> + Send;
}

pub const BATCH_CHUNK: usize = 20;

/// Build the `$batch` sub-request for a single op.
pub fn op_to_sub(index: usize, op: &MailOp) -> BatchSub {
    match op {
        MailOp::SetRead { id, is_read } => BatchSub {
            index,
            method: "PATCH",
            url: format!("/me/messages/{id}"),
            body: Some(serde_json::json!({ "isRead": is_read })),
        },
        MailOp::SetFlag { id, flagged } => BatchSub {
            index,
            method: "PATCH",
            url: format!("/me/messages/{id}"),
            body: Some(serde_json::json!({
                "flag": { "flagStatus": if *flagged { "flagged" } else { "notFlagged" } }
            })),
        },
        MailOp::SetCategories { id, categories } => BatchSub {
            index,
            method: "PATCH",
            url: format!("/me/messages/{id}"),
            body: Some(serde_json::json!({ "categories": categories })),
        },
        MailOp::SetInference { id, focused } => BatchSub {
            index,
            method: "PATCH",
            url: format!("/me/messages/{id}"),
            body: Some(serde_json::json!({
                "inferenceClassification": if *focused { "focused" } else { "other" }
            })),
        },
        MailOp::Move { id, dest_folder_id } => BatchSub {
            index,
            method: "POST",
            url: format!("/me/messages/{id}/move"),
            body: Some(serde_json::json!({ "destinationId": dest_folder_id })),
        },
        MailOp::HardDelete { id } => BatchSub {
            index,
            method: "DELETE",
            url: format!("/me/messages/{id}"),
            body: None,
        },
    }
}

#[derive(Debug, Clone)]
pub struct ItemOutcome {
    pub id: String,
    /// Set when a move reconciled to the destination copy's new id.
    pub new_id: Option<String>,
    /// `None` = confirmed by the server; `Some` = failed and rolled back.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct BatchOutcome {
    /// One per input op, in input order.
    pub outcomes: Vec<ItemOutcome>,
}

impl BatchOutcome {
    pub fn failed_ids(&self) -> Vec<String> {
        self.outcomes
            .iter()
            .filter(|o| o.error.is_some())
            .map(|o| o.id.clone())
            .collect()
    }
    pub fn failed_count(&self) -> usize {
        self.outcomes.iter().filter(|o| o.error.is_some()).count()
    }
}

/// Run a bulk action already applied optimistically to the store. `items`
/// pairs each op with the pre-op message snapshot for rollback. Sends in
/// ≤20 chunks; on **per-item** failure restores only that item's snapshot,
/// on move success reconciles the destination id, and leaves every confirmed
/// sibling applied. A whole-chunk transport error rolls back that chunk and
/// every not-yet-sent item — nothing stays applied without a 2xx.
pub async fn execute_batch(
    store: &Arc<dyn MailStore>,
    writer: &impl MailBatchWriter,
    items: Vec<(MailOp, Message)>,
) -> BatchOutcome {
    let mut outcomes: Vec<ItemOutcome> = items
        .iter()
        .map(|(op, _)| ItemOutcome {
            id: op.message_id().to_string(),
            new_id: None,
            error: None,
        })
        .collect();

    let mut chunk_start = 0;
    while chunk_start < items.len() {
        let chunk_end = (chunk_start + BATCH_CHUNK).min(items.len());
        let subs: Vec<BatchSub> = (chunk_start..chunk_end)
            .map(|i| op_to_sub(i, &items[i].0))
            .collect();

        match writer.execute_chunk(&subs).await {
            Ok(results) => {
                for i in chunk_start..chunk_end {
                    match results.iter().find(|r| r.index == i) {
                        Some(r) if (200..300).contains(&r.status) => {
                            if let MailOp::Move { id, dest_folder_id } = &items[i].0 {
                                // Reconcile to the destination copy's new id.
                                let new_id = r
                                    .body
                                    .get("id")
                                    .and_then(|v| v.as_str())
                                    .map(str::to_string);
                                if let Some(nid) = &new_id {
                                    let (id, dest, nid) =
                                        (id.clone(), dest_folder_id.clone(), nid.clone());
                                    let _ = store_call(store, move |s| {
                                        s.move_message(&id, &dest, Some(&nid))
                                    })
                                    .await;
                                }
                                outcomes[i].new_id = new_id;
                            }
                        }
                        other => {
                            // Missing response or non-2xx → roll back just this item.
                            let msg = match other {
                                Some(r) => format!(
                                    "{}: {}",
                                    r.status,
                                    r.body
                                        .pointer("/error/message")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("request failed")
                                ),
                                None => "no response in batch".to_string(),
                            };
                            restore_snapshot(store, items[i].1.clone()).await;
                            outcomes[i].error = Some(msg);
                        }
                    }
                }
            }
            Err(e) => {
                // Whole-chunk transport failure: roll back this chunk AND every
                // not-yet-sent item, then stop.
                let msg = e.to_string();
                for (i, item) in items.iter().enumerate().skip(chunk_start) {
                    restore_snapshot(store, item.1.clone()).await;
                    outcomes[i].error = Some(msg.clone());
                }
                break;
            }
        }
        chunk_start = chunk_end;
    }

    BatchOutcome { outcomes }
}

async fn restore_snapshot(store: &Arc<dyn MailStore>, snapshot: Message) {
    let _ = store_call(store, move |s| s.upsert_messages(&[snapshot])).await;
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::store::{Folder, MemoryMailStore, MessageSummary};

    #[derive(Default)]
    struct MockMailWriter {
        fail: bool,
        calls: Mutex<Vec<String>>,
    }

    impl MockMailWriter {
        fn failing() -> Self {
            Self {
                fail: true,
                calls: Mutex::new(Vec::new()),
            }
        }
        fn record(&self, what: &str) -> Result<(), SourceError> {
            self.calls.lock().unwrap().push(what.to_string());
            if self.fail {
                Err(SourceError::Http("boom".into()))
            } else {
                Ok(())
            }
        }
    }

    impl MailWriter for MockMailWriter {
        async fn set_read(&self, id: &str, v: bool) -> Result<(), SourceError> {
            self.record(&format!("read:{id}:{v}"))
        }
        async fn set_flag(&self, id: &str, v: bool) -> Result<(), SourceError> {
            self.record(&format!("flag:{id}:{v}"))
        }
        async fn set_categories(&self, id: &str, c: &[String]) -> Result<(), SourceError> {
            self.record(&format!("cats:{id}:{}", c.join(",")))
        }
        async fn set_inference(&self, id: &str, f: bool) -> Result<(), SourceError> {
            self.record(&format!("inf:{id}:{f}"))
        }
        async fn move_message(&self, id: &str, dest: &str) -> Result<String, SourceError> {
            self.record(&format!("move:{id}:{dest}"))?;
            Ok(format!("{id}-in-{dest}"))
        }
        async fn delete_message(&self, id: &str) -> Result<(), SourceError> {
            self.record(&format!("del:{id}"))
        }
    }

    fn store_with_message() -> Arc<dyn MailStore> {
        let store = MemoryMailStore::default();
        store
            .upsert_folders(&[
                Folder {
                    id: "inbox".into(),
                    display_name: "Inbox".into(),
                    well_known_name: Some("inbox".into()),
                    parent_id: None,
                },
                Folder {
                    id: "archive".into(),
                    display_name: "Archive".into(),
                    well_known_name: Some("archive".into()),
                    parent_id: None,
                },
            ])
            .unwrap();
        store
            .upsert_messages(&[Message {
                summary: MessageSummary {
                    id: "m1".into(),
                    folder_id: "inbox".into(),
                    subject: "Hello".into(),
                    from_name: "A".into(),
                    from_address: "a@x.com".into(),
                    received_at: 100,
                    preview: "hi".into(),
                    is_read: false,
                    has_attachments: false,
                    flag_status: "notFlagged".into(),
                    inference_classification: "focused".into(),
                    is_draft: false,
                },
                conversation_id: None,
                body_html: Some("<p>hi</p>".into()),
                body_content_type: Some("html".into()),
                internet_message_id: Some("m1@x".into()),
                categories: vec![],
            }])
            .unwrap();
        Arc::new(store)
    }

    fn msg(id: &str, folder: &str) -> Message {
        Message {
            summary: MessageSummary {
                id: id.into(),
                folder_id: folder.into(),
                subject: format!("subject {id}"),
                from_name: "A".into(),
                from_address: "a@x.com".into(),
                received_at: 100,
                preview: "hi".into(),
                is_read: false,
                has_attachments: false,
                flag_status: "notFlagged".into(),
                inference_classification: "focused".into(),
                is_draft: false,
            },
            conversation_id: None,
            body_html: None,
            body_content_type: None,
            internet_message_id: Some(format!("{id}@x")),
            categories: vec![],
        }
    }

    fn store_with_messages(ids: &[&str]) -> Arc<dyn MailStore> {
        let store = MemoryMailStore::default();
        store
            .upsert_folders(&[
                Folder {
                    id: "inbox".into(),
                    display_name: "Inbox".into(),
                    well_known_name: Some("inbox".into()),
                    parent_id: None,
                },
                Folder {
                    id: "archive".into(),
                    display_name: "Archive".into(),
                    well_known_name: Some("archive".into()),
                    parent_id: None,
                },
            ])
            .unwrap();
        let msgs: Vec<Message> = ids.iter().map(|id| msg(id, "inbox")).collect();
        store.upsert_messages(&msgs).unwrap();
        Arc::new(store)
    }

    /// Scripted batch writer: a per-index status/body function + a counter of
    /// how many chunks it was asked to send.
    struct MockMailBatchWriter {
        script: Box<dyn Fn(&BatchSub) -> SubResult + Send + Sync>,
        chunk_calls: Mutex<usize>,
        transport_fail: bool,
    }
    impl MockMailBatchWriter {
        fn new(f: impl Fn(&BatchSub) -> SubResult + Send + Sync + 'static) -> Self {
            Self {
                script: Box::new(f),
                chunk_calls: Mutex::new(0),
                transport_fail: false,
            }
        }
        fn transport_failing() -> Self {
            Self {
                script: Box::new(|s| SubResult {
                    index: s.index,
                    status: 200,
                    body: serde_json::json!({}),
                }),
                chunk_calls: Mutex::new(0),
                transport_fail: true,
            }
        }
    }
    impl MailBatchWriter for MockMailBatchWriter {
        async fn execute_chunk(&self, subs: &[BatchSub]) -> Result<Vec<SubResult>, SourceError> {
            *self.chunk_calls.lock().unwrap() += 1;
            if self.transport_fail {
                return Err(SourceError::Http("network down".into()));
            }
            Ok(subs.iter().map(|s| (self.script)(s)).collect())
        }
    }

    /// Apply a move optimistically (as the shell does before the batch fires)
    /// and return the pre-op snapshot for rollback.
    async fn apply_move(store: &Arc<dyn MailStore>, id: &str, dest: &str) -> (MailOp, Message) {
        let snapshot = store.get_message(id).unwrap().unwrap();
        store.move_message(id, dest, None).unwrap();
        (
            MailOp::Move {
                id: id.into(),
                dest_folder_id: dest.into(),
            },
            snapshot,
        )
    }

    #[tokio::test]
    async fn batch_partial_failure_rolls_back_only_failed_items() {
        let store = store_with_messages(&["m1", "m2", "m3"]);
        let items = vec![
            apply_move(&store, "m1", "archive").await,
            apply_move(&store, "m2", "archive").await,
            apply_move(&store, "m3", "archive").await,
        ];
        // Server accepts m1 (index 0) and m3 (index 2); rejects m2 (index 1).
        let writer = MockMailBatchWriter::new(|s| {
            if s.index == 1 {
                SubResult {
                    index: 1,
                    status: 403,
                    body: serde_json::json!({ "error": { "message": "quota exceeded" } }),
                }
            } else {
                SubResult {
                    index: s.index,
                    status: 201,
                    body: serde_json::json!({ "id": format!("moved-{}", s.index) }),
                }
            }
        });

        let out = execute_batch(&store, &writer, items).await;

        // m1 & m3 left the source and carry reconciled ids.
        assert!(store.get_message("m1").unwrap().is_none());
        assert_eq!(
            store
                .get_message("moved-0")
                .unwrap()
                .unwrap()
                .summary
                .folder_id,
            "archive"
        );
        assert!(store.get_message("moved-2").unwrap().is_some());
        // m2 rolled back: still present, original id, original folder.
        let m2 = store.get_message("m2").unwrap().unwrap();
        assert_eq!(m2.summary.folder_id, "inbox");
        // Only m2 reported failed.
        assert_eq!(out.failed_ids(), ["m2"]);
        assert_eq!(out.failed_count(), 1);
    }

    #[tokio::test]
    async fn batch_chunks_selections_over_twenty() {
        let ids: Vec<String> = (0..25).map(|i| format!("m{i}")).collect();
        let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let store = store_with_messages(&id_refs);
        let mut items = Vec::new();
        for id in &ids {
            items.push(apply_move(&store, id, "archive").await);
        }
        let writer = MockMailBatchWriter::new(|s| SubResult {
            index: s.index,
            status: 201,
            body: serde_json::json!({ "id": format!("a-{}", s.index) }),
        });

        let out = execute_batch(&store, &writer, items).await;

        // 25 items → exactly two chunks (20 + 5); all succeeded, none failed.
        assert_eq!(*writer.chunk_calls.lock().unwrap(), 2);
        assert_eq!(out.failed_count(), 0);
        assert!(store.get_message("a-24").unwrap().is_some());
    }

    #[tokio::test]
    async fn batch_transport_failure_rolls_back_everything() {
        let store = store_with_messages(&["m1", "m2"]);
        let items = vec![
            apply_move(&store, "m1", "archive").await,
            apply_move(&store, "m2", "archive").await,
        ];
        let writer = MockMailBatchWriter::transport_failing();

        let out = execute_batch(&store, &writer, items).await;

        assert_eq!(out.failed_count(), 2);
        // Both restored to the inbox.
        assert_eq!(
            store.get_message("m1").unwrap().unwrap().summary.folder_id,
            "inbox"
        );
        assert_eq!(
            store.get_message("m2").unwrap().unwrap().summary.folder_id,
            "inbox"
        );
    }

    #[tokio::test]
    async fn batch_mixed_ops_reconcile_and_apply() {
        let store = store_with_messages(&["m1", "m2"]);
        // m1: mark read (patch); m2: move.
        let read_snap = store.get_message("m1").unwrap().unwrap();
        store.set_read_state("m1", true).unwrap();
        let move_item = apply_move(&store, "m2", "archive").await;
        let items = vec![
            (
                MailOp::SetRead {
                    id: "m1".into(),
                    is_read: true,
                },
                read_snap,
            ),
            move_item,
        ];
        let writer = MockMailBatchWriter::new(|s| SubResult {
            index: s.index,
            status: 200,
            body: serde_json::json!({ "id": "m2-moved" }),
        });

        let out = execute_batch(&store, &writer, items).await;
        assert_eq!(out.failed_count(), 0);
        assert!(store.get_message("m1").unwrap().unwrap().summary.is_read);
        assert!(store.get_message("m2-moved").unwrap().is_some());
    }

    #[tokio::test]
    async fn success_paths_apply_and_confirm() {
        let store = store_with_message();
        let writer = MockMailWriter::default();

        execute_op(
            &store,
            &writer,
            MailOp::SetRead {
                id: "m1".into(),
                is_read: true,
            },
        )
        .await
        .unwrap();
        assert!(store.get_message("m1").unwrap().unwrap().summary.is_read);

        execute_op(
            &store,
            &writer,
            MailOp::SetFlag {
                id: "m1".into(),
                flagged: true,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            store
                .get_message("m1")
                .unwrap()
                .unwrap()
                .summary
                .flag_status,
            "flagged"
        );

        // Move reconciles to the server's new id in the destination folder.
        let new_id = execute_op(
            &store,
            &writer,
            MailOp::Move {
                id: "m1".into(),
                dest_folder_id: "archive".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(new_id, "m1-in-archive");
        assert!(store.get_message("m1").unwrap().is_none());
        let moved = store.get_message("m1-in-archive").unwrap().unwrap();
        assert_eq!(moved.summary.folder_id, "archive");
    }

    #[tokio::test]
    async fn failure_rolls_back_every_op_type() {
        // read
        let store = store_with_message();
        let before = store.get_message("m1").unwrap().unwrap();
        let err = execute_op(
            &store,
            &MockMailWriter::failing(),
            MailOp::SetRead {
                id: "m1".into(),
                is_read: true,
            },
        )
        .await;
        assert!(err.is_err());
        assert_eq!(
            store.get_message("m1").unwrap().unwrap(),
            before,
            "read rollback"
        );

        // flag
        let store = store_with_message();
        let before = store.get_message("m1").unwrap().unwrap();
        assert!(execute_op(
            &store,
            &MockMailWriter::failing(),
            MailOp::SetFlag {
                id: "m1".into(),
                flagged: true
            }
        )
        .await
        .is_err());
        assert_eq!(
            store.get_message("m1").unwrap().unwrap(),
            before,
            "flag rollback"
        );

        // categories
        let store = store_with_message();
        let before = store.get_message("m1").unwrap().unwrap();
        assert!(execute_op(
            &store,
            &MockMailWriter::failing(),
            MailOp::SetCategories {
                id: "m1".into(),
                categories: vec!["Red".into()]
            }
        )
        .await
        .is_err());
        assert_eq!(
            store.get_message("m1").unwrap().unwrap(),
            before,
            "categories rollback"
        );

        // inference
        let store = store_with_message();
        let before = store.get_message("m1").unwrap().unwrap();
        assert!(execute_op(
            &store,
            &MockMailWriter::failing(),
            MailOp::SetInference {
                id: "m1".into(),
                focused: false
            }
        )
        .await
        .is_err());
        assert_eq!(
            store.get_message("m1").unwrap().unwrap(),
            before,
            "inference rollback"
        );

        // move — the message must return to the inbox, unchanged and same id.
        let store = store_with_message();
        let before = store.get_message("m1").unwrap().unwrap();
        assert!(execute_op(
            &store,
            &MockMailWriter::failing(),
            MailOp::Move {
                id: "m1".into(),
                dest_folder_id: "archive".into()
            }
        )
        .await
        .is_err());
        assert!(
            store.get_message("m1").unwrap().is_some(),
            "move rollback resurrects id"
        );
        assert_eq!(
            store.get_message("m1").unwrap().unwrap(),
            before,
            "move rollback state"
        );

        // hard delete — a failed delete restores the row.
        let store = store_with_message();
        let before = store.get_message("m1").unwrap().unwrap();
        assert!(execute_op(
            &store,
            &MockMailWriter::failing(),
            MailOp::HardDelete { id: "m1".into() }
        )
        .await
        .is_err());
        assert_eq!(
            store.get_message("m1").unwrap().unwrap(),
            before,
            "hard-delete rollback resurrects the message"
        );
    }

    #[test]
    fn ops_deserialize_from_the_ui_camelcase_payloads() {
        // These are the exact JSON shapes the Svelte layer sends. Field names
        // are camelCase; they MUST map onto the snake_case Rust fields.
        type Check = fn(&MailOp) -> bool;
        let cases: &[(&str, Check)] = &[
            (
                r#"{"kind":"setRead","id":"m1","isRead":true}"#,
                |op| matches!(op, MailOp::SetRead { id, is_read: true } if id == "m1"),
            ),
            (r#"{"kind":"setFlag","id":"m1","flagged":true}"#, |op| {
                matches!(op, MailOp::SetFlag { flagged: true, .. })
            }),
            (
                r#"{"kind":"setInference","id":"m1","focused":false}"#,
                |op| matches!(op, MailOp::SetInference { focused: false, .. }),
            ),
            (
                r#"{"kind":"move","id":"m1","destFolderId":"archive"}"#,
                |op| matches!(op, MailOp::Move { dest_folder_id, .. } if dest_folder_id == "archive"),
            ),
            (
                r#"{"kind":"hardDelete","id":"m1"}"#,
                |op| matches!(op, MailOp::HardDelete { id } if id == "m1"),
            ),
        ];
        for (json, check) in cases {
            let op: MailOp = serde_json::from_str(json).unwrap_or_else(|e| {
                panic!("failed to deserialize {json}: {e}");
            });
            assert!(check(&op), "unexpected variant for {json}: {op:?}");
        }
    }

    #[tokio::test]
    async fn missing_message_is_an_error_not_a_panic() {
        let store = store_with_message();
        let err = execute_op(
            &store,
            &MockMailWriter::default(),
            MailOp::SetRead {
                id: "ghost".into(),
                is_read: true,
            },
        )
        .await;
        assert!(matches!(err, Err(OpError::NotFound(_))));
    }
}
