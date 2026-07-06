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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
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
