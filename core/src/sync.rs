//! Graph delta sync orchestration (per-folder).
//!
//! The algorithm runs against the `MailSource` trait so it is fully
//! unit-testable with `MockMailSource` — the same seam pattern as
//! `MailStore`/`TokenStore`. The real HTTP implementation is
//! `graph::GraphMailSource`.
//!
//! Correctness contract:
//! - **Backfill + baseline are one delta walk**: the initial request filters
//!   `receivedDateTime ge {cutoff}` (cutoff = the Nth-newest message's
//!   timestamp), so the walk both fills the window and terminates in the
//!   baseline `deltaLink`. No full-folder enumeration.
//! - **The cursor advances only at round end**: `sync_state.delta_link` is
//!   written exactly when the terminal `deltaLink` arrives, never for
//!   `nextLink`s. A crash mid-round replays from the previous cursor, which
//!   is safe because upserts are idempotent and deletes are no-op-if-missing.
//! - **Removals** (`@removed`, any reason — includes moves out of the folder)
//!   delete the local row.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;

use crate::store::{Folder, MailStore, MailStoreError, Message, SyncState};

pub const MAX_THROTTLE_RETRIES: u32 = 5;
/// Cap on a single throttle back-off, whatever Retry-After says.
pub const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    #[error("throttled by the server; retry after {retry_after:?}")]
    Throttled { retry_after: Duration },
    #[error("not signed in or token rejected")]
    Unauthorized,
    #[error("http error: {0}")]
    Http(String),
    #[error("protocol error: {0}")]
    Protocol(String),
}

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error(transparent)]
    Source(#[from] SourceError),
    #[error(transparent)]
    Store(#[from] MailStoreError),
    #[error("background task failed: {0}")]
    Join(String),
}

/// What to fetch next. `Url` carries a Graph-issued `nextLink` or a stored
/// `deltaLink` verbatim — those URLs already encode `$select`/`$filter`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeltaRequest {
    Initial { folder_id: String, since_epoch: i64 },
    Url(String),
}

#[derive(Debug, Clone)]
pub enum PageToken {
    /// `@odata.nextLink` — more pages in this round.
    Next(String),
    /// `@odata.deltaLink` — round complete; this is the next cursor.
    Delta(String),
}

#[derive(Debug, Clone)]
pub struct DeltaPage {
    pub messages: Vec<Message>,
    pub removed_ids: Vec<String>,
    pub token: PageToken,
}

/// The fetch surface sync depends on.
pub trait MailSource: Send + Sync {
    /// Timestamp of the Nth-most-recent message (the backfill window edge);
    /// "now" for an empty folder.
    fn backfill_cutoff(
        &self,
        folder_id: &str,
        n: u32,
    ) -> impl Future<Output = Result<i64, SourceError>> + Send;
    fn fetch_delta_page(
        &self,
        request: &DeltaRequest,
    ) -> impl Future<Output = Result<DeltaPage, SourceError>> + Send;
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SyncReport {
    /// True when this run established the baseline (no stored cursor).
    pub initial: bool,
    pub pages: u32,
    pub upserted: u64,
    pub removed: u64,
}

pub(crate) fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_secs() as i64
}

/// Run one sync round for a folder: initial backfill-plus-baseline when no
/// cursor is stored, incremental delta otherwise. The folder row is
/// (re-)upserted first so the messages FK always resolves.
pub async fn sync_folder(
    source: &impl MailSource,
    store: Arc<dyn MailStore>,
    folder: &Folder,
    backfill_count: u32,
) -> Result<SyncReport, SyncError> {
    let folder_id = folder.id.clone();
    {
        let folder = folder.clone();
        store_call(&store, move |s| s.upsert_folders(&[folder])).await?;
    }

    let state = {
        let fid = folder_id.clone();
        store_call(&store, move |s| s.get_sync_state(&fid)).await?
    };

    let (mut request, initial) = match state.and_then(|s| s.delta_link) {
        Some(url) => (DeltaRequest::Url(url), false),
        None => {
            let since =
                with_throttle_retry(|| source.backfill_cutoff(&folder_id, backfill_count)).await?;
            tracing::info!(
                cutoff = since,
                "no cursor stored; starting initial backfill walk"
            );
            (
                DeltaRequest::Initial {
                    folder_id: folder_id.clone(),
                    since_epoch: since,
                },
                true,
            )
        }
    };

    let mut report = SyncReport {
        initial,
        ..Default::default()
    };

    loop {
        let page = with_throttle_retry(|| source.fetch_delta_page(&request)).await?;
        report.pages += 1;
        report.upserted += page.messages.len() as u64;
        report.removed += page.removed_ids.len() as u64;

        // The delta is folder-scoped; pin every row to the synced folder so
        // the messages.folder_id foreign key always resolves.
        let mut messages = page.messages;
        for m in &mut messages {
            m.summary.folder_id = folder_id.clone();
        }
        store_call(&store, move |s| s.upsert_messages(&messages)).await?;

        let removed = page.removed_ids;
        store_call(&store, move |s| {
            for id in &removed {
                s.delete_message(id)?;
            }
            Ok(())
        })
        .await?;

        match page.token {
            PageToken::Next(url) => request = DeltaRequest::Url(url),
            PageToken::Delta(url) => {
                // The only place the cursor is written: round complete.
                let new_state = SyncState {
                    folder_id: folder_id.clone(),
                    delta_link: Some(url),
                    last_synced_at: Some(now_epoch()),
                };
                store_call(&store, move |s| s.set_sync_state(&new_state)).await?;
                break;
            }
        }
    }

    tracing::info!(
        initial = report.initial,
        pages = report.pages,
        upserted = report.upserted,
        removed = report.removed,
        "sync round complete; cursor stored"
    );
    Ok(report)
}

/// Retry `op` on `Throttled`, honoring (capped) Retry-After, up to
/// `MAX_THROTTLE_RETRIES` attempts. Lives here rather than in the HTTP layer
/// so `MockMailSource` can exercise it.
async fn with_throttle_retry<T, F, Fut>(mut op: F) -> Result<T, SourceError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, SourceError>>,
{
    let mut attempts = 0;
    loop {
        match op().await {
            Err(SourceError::Throttled { retry_after }) if attempts < MAX_THROTTLE_RETRIES => {
                attempts += 1;
                let wait = retry_after.min(MAX_RETRY_AFTER);
                tracing::warn!(?wait, attempt = attempts, "throttled; backing off");
                tokio::time::sleep(wait).await;
            }
            other => return other,
        }
    }
}

async fn store_call<T, F>(store: &Arc<dyn MailStore>, f: F) -> Result<T, SyncError>
where
    F: FnOnce(&dyn MailStore) -> Result<T, MailStoreError> + Send + 'static,
    T: Send + 'static,
{
    let store = Arc::clone(store);
    tokio::task::spawn_blocking(move || f(store.as_ref()))
        .await
        .map_err(|e| SyncError::Join(e.to_string()))?
        .map_err(SyncError::from)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::*;
    use crate::store::{MemoryMailStore, MessageSummary};

    fn inbox() -> Folder {
        Folder {
            id: "inbox-id".into(),
            display_name: "Inbox".into(),
            well_known_name: Some("inbox".into()),
            parent_id: None,
        }
    }

    struct MockMailSource {
        cutoff: i64,
        pages: Mutex<VecDeque<Result<DeltaPage, SourceError>>>,
        requests: Mutex<Vec<DeltaRequest>>,
    }

    impl MockMailSource {
        fn new(pages: Vec<Result<DeltaPage, SourceError>>) -> Self {
            Self {
                cutoff: 1_000,
                pages: Mutex::new(pages.into()),
                requests: Mutex::new(Vec::new()),
            }
        }

        fn requests(&self) -> Vec<DeltaRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl MailSource for MockMailSource {
        async fn backfill_cutoff(&self, _folder_id: &str, _n: u32) -> Result<i64, SourceError> {
            Ok(self.cutoff)
        }

        async fn fetch_delta_page(&self, request: &DeltaRequest) -> Result<DeltaPage, SourceError> {
            self.requests.lock().unwrap().push(request.clone());
            self.pages
                .lock()
                .unwrap()
                .pop_front()
                .expect("mock ran out of scripted pages")
        }
    }

    fn msg(id: &str, received_at: i64, is_read: bool) -> Message {
        Message {
            summary: MessageSummary {
                id: id.into(),
                folder_id: String::new(), // sync must pin this to the folder
                subject: format!("subject {id}"),
                from_name: "Sender".into(),
                from_address: "s@example.com".into(),
                received_at,
                preview: String::new(),
                is_read,
                has_attachments: false,
                flag_status: "notFlagged".to_string(),
                inference_classification: "focused".to_string(),
                is_draft: false,
            },
            conversation_id: None,
            body_html: None,
            body_content_type: None,
            internet_message_id: None,
            categories: Vec::new(),
        }
    }

    fn page(messages: Vec<Message>, removed: Vec<&str>, token: PageToken) -> DeltaPage {
        DeltaPage {
            messages,
            removed_ids: removed.into_iter().map(String::from).collect(),
            token,
        }
    }

    #[tokio::test]
    async fn initial_backfill_pages_to_delta_link_and_stores_cursor() {
        let source = MockMailSource::new(vec![
            Ok(page(
                vec![msg("m1", 300, false), msg("m2", 200, false)],
                vec![],
                PageToken::Next("https://graph/next-1".into()),
            )),
            Ok(page(
                vec![msg("m3", 100, false)],
                vec![],
                PageToken::Delta("https://graph/delta-1".into()),
            )),
        ]);
        let store: Arc<dyn MailStore> = Arc::new(MemoryMailStore::default());

        let report = sync_folder(&source, Arc::clone(&store), &inbox(), 200)
            .await
            .unwrap();

        assert!(report.initial);
        assert_eq!(report.pages, 2);
        assert_eq!(report.upserted, 3);
        assert_eq!(report.removed, 0);

        // All three landed, newest first, pinned to the synced folder.
        let listed = store.list_messages("inbox-id", 10, 0).unwrap();
        assert_eq!(
            listed.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["m1", "m2", "m3"]
        );

        // Cursor stored exactly once, at round end.
        let state = store.get_sync_state("inbox-id").unwrap().unwrap();
        assert_eq!(state.delta_link.as_deref(), Some("https://graph/delta-1"));
        assert!(state.last_synced_at.is_some());

        // First request was the filtered Initial; the second followed nextLink.
        assert_eq!(
            source.requests(),
            vec![
                DeltaRequest::Initial {
                    folder_id: "inbox-id".into(),
                    since_epoch: 1_000
                },
                DeltaRequest::Url("https://graph/next-1".into()),
            ]
        );
    }

    #[tokio::test]
    async fn incremental_sync_uses_stored_cursor_and_applies_changes() {
        let store: Arc<dyn MailStore> = Arc::new(MemoryMailStore::default());
        // Seed: folder, two messages (one unread), and a stored cursor.
        store
            .upsert_folders(&[Folder {
                id: "inbox-id".into(),
                display_name: "Inbox".into(),
                well_known_name: Some("inbox".into()),
                parent_id: None,
            }])
            .unwrap();
        let mut m1 = msg("m1", 300, false);
        m1.summary.folder_id = "inbox-id".into();
        let mut m2 = msg("m2", 200, false);
        m2.summary.folder_id = "inbox-id".into();
        store.upsert_messages(&[m1, m2]).unwrap();
        store
            .set_sync_state(&SyncState {
                folder_id: "inbox-id".into(),
                delta_link: Some("https://graph/delta-1".into()),
                last_synced_at: Some(1),
            })
            .unwrap();

        // Delta: m1 flips to read, m2 removed.
        let source = MockMailSource::new(vec![Ok(page(
            vec![msg("m1", 300, true)],
            vec!["m2"],
            PageToken::Delta("https://graph/delta-2".into()),
        ))]);

        let report = sync_folder(&source, Arc::clone(&store), &inbox(), 200)
            .await
            .unwrap();

        assert!(!report.initial);
        assert_eq!(report.upserted, 1);
        assert_eq!(report.removed, 1);

        // The stored cursor was sent verbatim; no cutoff probe happened.
        assert_eq!(
            source.requests(),
            vec![DeltaRequest::Url("https://graph/delta-1".into())]
        );

        // isRead flip updated in place (no duplicate), m2 gone.
        let listed = store.list_messages("inbox-id", 10, 0).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "m1");
        assert!(listed[0].is_read);

        let state = store.get_sync_state("inbox-id").unwrap().unwrap();
        assert_eq!(state.delta_link.as_deref(), Some("https://graph/delta-2"));
    }

    #[tokio::test]
    async fn throttled_request_is_retried_and_succeeds() {
        let source = MockMailSource::new(vec![
            Err(SourceError::Throttled {
                retry_after: Duration::ZERO,
            }),
            Ok(page(
                vec![msg("m1", 300, false)],
                vec![],
                PageToken::Delta("https://graph/delta-1".into()),
            )),
        ]);
        let store: Arc<dyn MailStore> = Arc::new(MemoryMailStore::default());

        let report = sync_folder(&source, Arc::clone(&store), &inbox(), 200)
            .await
            .unwrap();
        assert_eq!(report.upserted, 1);

        // Same request sent twice: throttled, then retried.
        let requests = source.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0], requests[1]);
    }

    #[tokio::test]
    async fn cursor_is_not_stored_when_a_round_fails_mid_walk() {
        let source = MockMailSource::new(vec![
            Ok(page(
                vec![msg("m1", 300, false)],
                vec![],
                PageToken::Next("https://graph/next-1".into()),
            )),
            Err(SourceError::Http("boom".into())),
        ]);
        let store: Arc<dyn MailStore> = Arc::new(MemoryMailStore::default());

        let err = sync_folder(&source, Arc::clone(&store), &inbox(), 200)
            .await
            .unwrap_err();
        assert!(matches!(err, SyncError::Source(SourceError::Http(_))));

        // Partial upserts are fine (idempotent replay later), but the cursor
        // must not have advanced.
        assert!(store.get_sync_state("inbox-id").unwrap().is_none());
    }
}
