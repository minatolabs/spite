use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use spite_core::auth::token_store::KeyringTokenStore;
use spite_core::auth::{Account, Authenticator, DeviceCodePrompt};
use spite_core::compose::{self, ComposeMode, Draft, EmailAddress};
use spite_core::config::AppConfig;
use spite_core::graph::{DraftHandle, GraphMailSource, ServerHit};
use spite_core::ops::{execute_batch, execute_op, MailOp};
use spite_core::sanitize::sanitize_html;
use spite_core::settings::{is_offered_preset, AutomaticReplies, MailboxSettings, MasterCategory};
use spite_core::store::{
    build_kql, Folder, MailStore, MailStoreError, Message, MessageSummary, SavedSearch,
    SearchFilters, SearchHit, SqliteMailStore, SyncState,
};
use spite_core::sync::{sync_folder as run_sync_folder, SyncReport};
use tauri::{AppHandle, Emitter, Manager, State};

/// Pending compose-window parameters, keyed by window label. Avoids pushing
/// Graph message ids (or whole drafts) through URL query encoding.
#[derive(Clone)]
enum ComposeParams {
    /// Fresh compose/reply/forward built from an original message.
    Original {
        mode: String,
        message_id: Option<String>,
    },
    /// A draft coming back from Undo or from a failed send.
    Restore { draft: Draft, error: Option<String> },
}

#[derive(Default)]
struct ComposeRegistry {
    params: Mutex<HashMap<String, ComposeParams>>,
    label_seq: std::sync::atomic::AtomicU64,
}

/// Undo-send queue: drafts wait out the countdown here. The timer task
/// claims a draft by removing it — an entry that is gone was either undone
/// or already sent, so claim-by-remove is the cancellation mechanism.
#[derive(Default)]
struct SendQueue {
    pending: Mutex<HashMap<u64, Draft>>,
    next_id: std::sync::atomic::AtomicU64,
}

#[derive(Debug, Clone, Serialize)]
struct SendQueuedEvent {
    id: u64,
    subject: String,
    recipients: usize,
    /// Unix millis when the send fires; the toast counts down to this.
    deadline_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
struct SendResultEvent {
    id: u64,
    error: Option<String>,
}

/// Undoable mail-management ops (archive/move/soft-delete/hard-delete) wait
/// out a countdown here, exactly like `SendQueue`. Each entry keeps the
/// pre-op message snapshot so Undo (or a failed Graph call on lapse) can
/// restore the optimistic local change. Claim-by-remove is cancellation.
/// One queue entry = one whole action (a single op is a 1-element vec, a bulk
/// action is N). Each item pairs its op with the pre-op snapshot for rollback.
#[derive(Default)]
struct OpQueue {
    pending: Mutex<HashMap<u64, Vec<(MailOp, Message)>>>,
    next_id: std::sync::atomic::AtomicU64,
}

#[derive(Debug, Clone, Serialize)]
struct OpQueuedEvent {
    id: u64,
    /// "archive" | "delete" | "move" — drives the toast label.
    label: String,
    subject: String,
    deadline_ms: u64,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn open_compose_window(app: &AppHandle, params: ComposeParams) -> Result<(), String> {
    let registry = app.state::<ComposeRegistry>();
    let seq = registry
        .label_seq
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let label = format!("compose-{}-{seq}", now_ms());
    registry
        .params
        .lock()
        .unwrap()
        .insert(label.clone(), params);
    tauri::WebviewWindowBuilder::new(
        app,
        &label,
        tauri::WebviewUrl::App(format!("index.html?compose=1&label={label}").into()),
    )
    .title("Compose — Spite")
    .inner_size(760.0, 680.0)
    .build()
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Store access is blocking (SQLite behind a mutex) — keep it off the async
/// runtime, mirroring the pattern in spite-core.
async fn store_call<T, F>(store: &Arc<dyn MailStore>, f: F) -> Result<T, String>
where
    F: FnOnce(&dyn MailStore) -> Result<T, MailStoreError> + Send + 'static,
    T: Send + 'static,
{
    let store = Arc::clone(store);
    tauri::async_runtime::spawn_blocking(move || f(store.as_ref()))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn sign_in(app: AppHandle, auth: State<'_, Arc<Authenticator>>) -> Result<Account, String> {
    auth.sign_in(|prompt: DeviceCodePrompt| {
        let _ = app.emit("auth:device-code", &prompt);
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn silent_sign_in(auth: State<'_, Arc<Authenticator>>) -> Result<Account, String> {
    auth.silent_sign_in().await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn sign_out(auth: State<'_, Arc<Authenticator>>) -> Result<(), String> {
    auth.sign_out().await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn list_folders(store: State<'_, Arc<dyn MailStore>>) -> Result<Vec<Folder>, String> {
    store_call(&store, |s| s.list_folders()).await
}

#[tauri::command]
async fn list_messages(
    folder_id: String,
    limit: u32,
    offset: u32,
    store: State<'_, Arc<dyn MailStore>>,
) -> Result<Vec<MessageSummary>, String> {
    store_call(&store, move |s| s.list_messages(&folder_id, limit, offset)).await
}

#[tauri::command]
async fn unread_counts(store: State<'_, Arc<dyn MailStore>>) -> Result<Vec<(String, u32)>, String> {
    store_call(&store, |s| s.unread_counts()).await
}

#[tauri::command]
async fn get_message(
    id: String,
    store: State<'_, Arc<dyn MailStore>>,
) -> Result<Option<Message>, String> {
    store_call(&store, move |s| s.get_message(&id)).await
}

#[derive(Debug, Serialize)]
struct MessageBody {
    body: String,
    /// "html" (already sanitized) or "text" (rendered as text, never HTML).
    content_type: String,
}

/// Lazy body load: cache hit returns the stored (sanitized) body; a miss
/// fetches from Graph, sanitizes HTML in Rust, caches, then returns. The
/// webview never receives unsanitized message HTML.
#[tauri::command]
async fn fetch_message_body(
    id: String,
    auth: State<'_, Arc<Authenticator>>,
    store: State<'_, Arc<dyn MailStore>>,
) -> Result<MessageBody, String> {
    // `None` = not in the local store (e.g. a server-search hit outside the
    // sync window) — still fetchable, just not cached below.
    let cached = {
        let id = id.clone();
        store_call(&store, move |s| s.get_message(&id)).await?
    };

    if let Some(body) = cached.as_ref().and_then(|m| m.body_html.clone()) {
        return Ok(MessageBody {
            body,
            content_type: cached
                .and_then(|m| m.body_content_type)
                .unwrap_or_else(|| "html".to_string()),
        });
    }

    let source = GraphMailSource::new(Arc::clone(&auth));
    let fetched = source
        .fetch_message_body(&id)
        .await
        .map_err(|e| format!("body not downloaded: {e}"))?;
    let (body, content_type) = if fetched.content_type == "html" {
        (sanitize_html(&fetched.content), "html".to_string())
    } else {
        (fetched.content, "text".to_string())
    };

    // Cache only for locally-synced messages; a server-search hit outside
    // the sync window (cached == None) is returned without persisting.
    if cached.is_some() {
        let (id, body, ct) = (id.clone(), body.clone(), content_type.clone());
        store_call(&store, move |s| s.set_message_body(&id, &body, &ct)).await?;
    }
    Ok(MessageBody { body, content_type })
}

/// Refresh the folder list from Graph into the store; returns the fresh list.
#[tauri::command]
async fn sync_folders(
    auth: State<'_, Arc<Authenticator>>,
    store: State<'_, Arc<dyn MailStore>>,
) -> Result<Vec<Folder>, String> {
    let source = GraphMailSource::new(Arc::clone(&auth));
    let folders = source.list_all_folders().await.map_err(|e| e.to_string())?;
    {
        let folders = folders.clone();
        store_call(&store, move |s| s.upsert_folders(&folders)).await?;
    }
    Ok(folders)
}

/// One delta-sync round for a folder (UI-driven: folder open, window focus,
/// interval — replaces the Phase 3 provisional startup sync).
#[tauri::command]
async fn sync_folder(
    folder_id: String,
    auth: State<'_, Arc<Authenticator>>,
    store: State<'_, Arc<dyn MailStore>>,
    config: State<'_, AppConfig>,
) -> Result<SyncReport, String> {
    let folder = store_call(&store, |s| s.list_folders())
        .await?
        .into_iter()
        .find(|f| f.id == folder_id)
        .ok_or_else(|| "unknown folder".to_string())?;
    let source = GraphMailSource::new(Arc::clone(&auth));
    run_sync_folder(&source, Arc::clone(&store), &folder, config.backfill_count)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_sync_status(
    folder_id: String,
    store: State<'_, Arc<dyn MailStore>>,
) -> Result<Option<SyncState>, String> {
    store_call(&store, move |s| s.get_sync_state(&folder_id)).await
}

#[tauri::command]
async fn get_sender_pref(
    address: String,
    store: State<'_, Arc<dyn MailStore>>,
) -> Result<bool, String> {
    store_call(&store, move |s| s.get_sender_pref(&address)).await
}

#[tauri::command]
async fn set_sender_pref(
    address: String,
    allow_remote_images: bool,
    store: State<'_, Arc<dyn MailStore>>,
) -> Result<(), String> {
    store_call(&store, move |s| {
        s.set_sender_pref(&address, allow_remote_images)
    })
    .await
}

/// Open a compose window (new/reply/replyAll/forward). Window parameters
/// travel through the registry; the URL carries only the label.
#[tauri::command]
async fn open_compose(
    app: AppHandle,
    mode: String,
    message_id: Option<String>,
) -> Result<(), String> {
    open_compose_window(&app, ComposeParams::Original { mode, message_id })
}

#[derive(Debug, Serialize)]
struct ComposeContext {
    mode: String,
    to: Vec<EmailAddress>,
    cc: Vec<EmailAddress>,
    subject: String,
    /// Sanitized quote of the original (reply/forward), ready for the editor.
    quoted_html: Option<String>,
    in_reply_to: Option<String>,
    references: Vec<String>,
    signature: Option<String>,
    /// True when the reply context had to be built from local data only
    /// (offline): threading headers may be missing and reply-all may have
    /// degraded to reply.
    degraded: bool,
    /// A restored draft (Undo or failed send) — when set, the composer
    /// loads these fields verbatim and ignores the ones above.
    restored: Option<Draft>,
    /// Why the draft came back, if it came back from a failure.
    restore_error: Option<String>,
    /// Server draft id (Phase 7): present when a `createReply*` draft was
    /// created — the composer then autosaves + sends via it. `None` means the
    /// offline MIME path (Phase 5) is used.
    draft_id: Option<String>,
}

impl ComposeContext {
    fn empty(mode: String, signature: Option<String>) -> Self {
        Self {
            mode,
            to: vec![],
            cc: vec![],
            subject: String::new(),
            quoted_html: None,
            in_reply_to: None,
            references: vec![],
            signature,
            degraded: false,
            restored: None,
            restore_error: None,
            draft_id: None,
        }
    }
}

#[tauri::command]
async fn get_compose_context(
    label: String,
    registry: State<'_, ComposeRegistry>,
    auth: State<'_, Arc<Authenticator>>,
    store: State<'_, Arc<dyn MailStore>>,
) -> Result<ComposeContext, String> {
    let params = registry
        .params
        .lock()
        .unwrap()
        .get(&label)
        .cloned()
        .ok_or_else(|| "unknown compose window".to_string())?;
    let (mode_str, message_id) = match params {
        ComposeParams::Restore { draft, error } => {
            return Ok(ComposeContext {
                restored: Some(draft),
                restore_error: error,
                ..ComposeContext::empty("restore".to_string(), None)
            });
        }
        ComposeParams::Original { mode, message_id } => (mode, message_id),
    };
    let mode = match mode_str.as_str() {
        "reply" => ComposeMode::Reply,
        "replyAll" => ComposeMode::ReplyAll,
        "forward" => ComposeMode::Forward,
        _ => ComposeMode::New,
    };
    let account = auth
        .current_account()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "not signed in".to_string())?;

    let signature_kind = if matches!(mode, ComposeMode::New) {
        "new"
    } else {
        "reply"
    };
    let signature = {
        let upn = account.upn.clone();
        let kind = signature_kind.to_string();
        store_call(&store, move |s| s.get_signature(&upn, &kind)).await?
    };

    if matches!(mode, ComposeMode::New) {
        return Ok(ComposeContext::empty(mode_str, signature));
    }

    let message_id = message_id.ok_or_else(|| "missing original message id".to_string())?;

    // Original message from the local store (headers + maybe cached body).
    let local = {
        let id = message_id.clone();
        store_call(&store, move |s| s.get_message(&id)).await?
    };

    // Reply context from Graph; degrade to local-only data offline.
    let source = GraphMailSource::new(Arc::clone(&auth));
    let (ctx, degraded) = match source.fetch_reply_context(&message_id).await {
        Ok(ctx) => (ctx, false),
        Err(e) => {
            tracing::warn!(error = %e, "reply context fetch failed; degrading to local data");
            let Some(local) = &local else {
                return Err("original message unavailable".to_string());
            };
            let s = &local.summary;
            (
                compose::ReplyContext {
                    internet_message_id: None,
                    references: vec![],
                    from: Some(EmailAddress {
                        name: s.from_name.clone(),
                        address: s.from_address.clone(),
                    }),
                    reply_to: vec![],
                    to: vec![],
                    cc: vec![],
                    subject: s.subject.clone(),
                    received_at: s.received_at,
                },
                true,
            )
        }
    };

    let effective_mode = if degraded && matches!(mode, ComposeMode::ReplyAll) {
        // Without the original recipient lists, reply-all can only degrade
        // to reply — surfaced to the user via `degraded`.
        ComposeMode::Reply
    } else {
        mode
    };
    let (to, cc) = compose::reply_recipients(&ctx, &account.upn, effective_mode);

    let subject = match mode {
        ComposeMode::Forward => compose::forward_subject(&ctx.subject),
        _ => compose::reply_subject(&ctx.subject),
    };

    // Quote from the cached (sanitized) body when available, else fetch it.
    let (from_name, from_address) = ctx
        .from
        .as_ref()
        .map(|f| (f.name.clone(), f.address.clone()))
        .unwrap_or_default();
    let body_html = match local.as_ref().and_then(|m| m.body_html.clone()) {
        Some(b) => Some(b),
        None => match source.fetch_message_body(&message_id).await {
            Ok(fetched) if fetched.content_type == "html" => Some(sanitize_html(&fetched.content)),
            Ok(fetched) => Some(format!("<pre>{}</pre>", {
                // Plain-text original: escape into a pre block for quoting.
                fetched
                    .content
                    .replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;")
            })),
            Err(_) => None,
        },
    };
    let quoted_html =
        body_html.map(|b| compose::quote_html(&b, &from_name, &from_address, ctx.received_at));

    let (in_reply_to, references) = match (&effective_mode, &ctx.internet_message_id) {
        (ComposeMode::Reply | ComposeMode::ReplyAll, Some(mid)) => (
            Some(compose::normalize_msgid(mid)),
            compose::build_references(&ctx.references, mid),
        ),
        _ => (None, vec![]), // forwards don't thread; degraded replies can't
    };

    // Phase 7: when online, create a server draft (createReply/ReplyAll/
    // Forward) — Exchange handles the quote + threading, and the draft is
    // editable across sessions. If it fails (offline), fall back to the MIME
    // path below (draft_id stays None).
    let draft_id = if degraded {
        None
    } else {
        source
            .create_reply_draft(&message_id, &mode_str)
            .await
            .ok()
            .map(|d| d.id)
    };

    Ok(ComposeContext {
        to,
        cc,
        subject,
        quoted_html,
        in_reply_to,
        references,
        degraded,
        draft_id,
        ..ComposeContext::empty(mode_str, signature)
    })
}

/// Undo-send model: validate the draft, park it in the queue, and schedule
/// the real send after the configured countdown. Returns as soon as the
/// draft is queued — the composer closes immediately and the main window
/// shows the "Sending… Undo" toast (driven by the `send:*` events).
#[tauri::command]
async fn queue_send(
    app: AppHandle,
    draft: Draft,
    queue: State<'_, Arc<SendQueue>>,
    auth: State<'_, Arc<Authenticator>>,
    config: State<'_, AppConfig>,
) -> Result<(), String> {
    // Validate now, while the composer is still open to show the error:
    // recipient presence, attachment size/encoding, MIME buildability.
    let account = auth
        .current_account()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "not signed in".to_string())?;
    let from = EmailAddress {
        name: account.display_name,
        address: account.upn,
    };
    compose::build_mime(&draft, &from).map_err(|e| e.to_string())?;

    let id = queue
        .next_id
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let delay_secs = config.undo_send_seconds.min(120);
    let event = SendQueuedEvent {
        id,
        subject: draft.subject.clone(),
        recipients: draft.to.len() + draft.cc.len() + draft.bcc.len(),
        deadline_ms: now_ms() + u64::from(delay_secs) * 1000,
    };
    queue.pending.lock().unwrap().insert(id, draft);
    let _ = app.emit("send:queued", &event);

    // The composer closes itself right after this returns; hand focus back
    // to the main window so the countdown toast is in view.
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.set_focus();
    }

    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        if delay_secs > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(u64::from(delay_secs))).await;
        }
        perform_send(&handle, id).await;
    });
    Ok(())
}

/// Cancel a queued send and hand the draft back in a fresh composer window.
/// Claiming the draft out of the queue is the cancellation: the timer task
/// finds nothing to send.
#[tauri::command]
async fn undo_send(
    app: AppHandle,
    id: u64,
    queue: State<'_, Arc<SendQueue>>,
) -> Result<(), String> {
    let draft = queue
        .pending
        .lock()
        .unwrap()
        .remove(&id)
        .ok_or_else(|| "too late — already sent".to_string())?;
    open_compose_window(&app, ComposeParams::Restore { draft, error: None })
}

/// Fire a queued send. On failure the draft is never dropped: it reopens in
/// a composer window with the error, and the toast reports what happened.
async fn perform_send(app: &AppHandle, id: u64) {
    let queue = app.state::<Arc<SendQueue>>();
    // Claim by removal; a missing entry means the user undid it.
    let Some(draft) = queue.pending.lock().unwrap().remove(&id) else {
        return;
    };

    let auth = Arc::clone(&app.state::<Arc<Authenticator>>());
    let result: Result<(), String> = async {
        let account = auth
            .current_account()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "not signed in".to_string())?;
        let from = EmailAddress {
            name: account.display_name,
            address: account.upn,
        };
        let mime = compose::build_mime(&draft, &from).map_err(|e| e.to_string())?;
        GraphMailSource::new(Arc::clone(&auth))
            .send_mime(&mime)
            .await
            .map_err(|e| e.to_string())
    }
    .await;

    match result {
        Ok(()) => {
            let _ = app.emit("send:sent", &SendResultEvent { id, error: None });
            let recipients: Vec<(String, String)> = draft
                .to
                .iter()
                .chain(&draft.cc)
                .chain(&draft.bcc)
                .map(|a| (a.address.clone(), a.name.clone()))
                .collect();
            let now = (now_ms() / 1000) as i64;
            let store = Arc::clone(&app.state::<Arc<dyn MailStore>>());
            // History is best-effort; the mail is already sent.
            let _ = store_call(&store, move |s| s.record_recipients(&recipients, now)).await;
        }
        Err(e) => {
            tracing::warn!(error = %e, "queued send failed; reopening draft");
            let _ = app.emit(
                "send:failed",
                &SendResultEvent {
                    id,
                    error: Some(e.clone()),
                },
            );
            let _ = open_compose_window(
                app,
                ComposeParams::Restore {
                    draft,
                    error: Some(format!("send failed: {e}")),
                },
            );
        }
    }
}

/// Instant local search (FTS5): ranked, highlighted, offline. Empty query
/// with filters = filtered browse.
#[tauri::command]
async fn search_local(
    query: String,
    filters: SearchFilters,
    limit: u32,
    store: State<'_, Arc<dyn MailStore>>,
) -> Result<Vec<SearchHit>, String> {
    store_call(&store, move |s| s.search(&query, &filters, limit)).await
}

/// Deep server search over the whole mailbox, deduped against local rows by
/// Graph id and internetMessageId. `Mail.Read` only.
#[tauri::command]
async fn search_server(
    query: String,
    filters: SearchFilters,
    auth: State<'_, Arc<Authenticator>>,
    store: State<'_, Arc<dyn MailStore>>,
) -> Result<Vec<ServerHit>, String> {
    let kql = build_kql(&query, &filters);
    if kql.is_empty() {
        return Ok(vec![]);
    }
    let source = GraphMailSource::new(Arc::clone(&auth));
    let hits = source
        .search_messages(&kql, 25)
        .await
        .map_err(|e| e.to_string())?;
    store_call(&store, move |s| {
        let mut fresh = Vec::new();
        for hit in hits {
            if !s.message_exists(&hit.summary.id, hit.internet_message_id.as_deref())? {
                fresh.push(hit);
            }
        }
        Ok(fresh)
    })
    .await
}

#[tauri::command]
async fn list_saved_searches(
    store: State<'_, Arc<dyn MailStore>>,
) -> Result<Vec<SavedSearch>, String> {
    store_call(&store, |s| s.list_saved_searches()).await
}

#[tauri::command]
async fn save_search(
    name: String,
    query: String,
    filters: SearchFilters,
    store: State<'_, Arc<dyn MailStore>>,
) -> Result<i64, String> {
    let filters_json = serde_json::to_string(&filters).map_err(|e| e.to_string())?;
    store_call(&store, move |s| s.save_search(&name, &query, &filters_json)).await
}

#[tauri::command]
async fn delete_saved_search(id: i64, store: State<'_, Arc<dyn MailStore>>) -> Result<(), String> {
    store_call(&store, move |s| s.delete_saved_search(id)).await
}

/// Keyboard-shortcut overrides from config.json (UI merges over defaults).
#[tauri::command]
fn get_keymap(config: State<'_, AppConfig>) -> std::collections::HashMap<String, String> {
    config.keymap.clone()
}

/// Dwell (ms) before an opened unread message auto-marks read; 0 disables.
#[tauri::command]
fn get_auto_read_dwell(config: State<'_, AppConfig>) -> u32 {
    config.auto_read_dwell_ms
}

/// Repair rows whose summary a partial delta event blanked: re-fetch full
/// summaries from Graph and upsert (the upsert guard lets the full data win).
/// Returns the number of rows repaired.
#[tauri::command]
async fn repair_summaries(
    auth: State<'_, Arc<Authenticator>>,
    store: State<'_, Arc<dyn MailStore>>,
) -> Result<usize, String> {
    let ids = store_call(&store, |s| s.broken_summary_ids(500)).await?;
    if ids.is_empty() {
        return Ok(0);
    }
    let msgs = GraphMailSource::new(Arc::clone(&auth))
        .fetch_summaries(&ids)
        .await
        .map_err(|e| e.to_string())?;
    let n = msgs.len();
    store_call(&store, move |s| s.upsert_messages(&msgs)).await?;
    tracing::info!(
        repaired = n,
        candidates = ids.len(),
        "repaired blanked summaries"
    );
    Ok(n)
}

// --- Phase 7 mail management ---

/// Immediate optimistic op (read/flag/categories/inference): apply locally,
/// call Graph, roll back on failure. The UI updates its own state instantly
/// and reverts if this returns an error.
#[tauri::command]
async fn apply_op(
    op: MailOp,
    auth: State<'_, Arc<Authenticator>>,
    store: State<'_, Arc<dyn MailStore>>,
) -> Result<String, String> {
    let writer = GraphMailSource::new(Arc::clone(&auth));
    execute_op(&store, &writer, op)
        .await
        .map_err(|e| e.to_string())
}

/// Single undoable op — a thin wrapper over the bulk path with a 1-op vec.
#[tauri::command]
async fn queue_op(
    app: AppHandle,
    op: MailOp,
    label: String,
    queue: State<'_, Arc<OpQueue>>,
    store: State<'_, Arc<dyn MailStore>>,
    config: State<'_, AppConfig>,
) -> Result<(), String> {
    queue_items(
        &app,
        vec![op],
        label,
        &queue,
        &store,
        config.undo_send_seconds,
    )
    .await
}

/// Bulk undoable action: apply every op locally now, show ONE undo toast, and
/// fire a single deferred `$batch` on lapse. Nothing hits Graph during the
/// window; Undo cancels every chunk before any fires.
#[tauri::command]
async fn queue_bulk_op(
    app: AppHandle,
    ops: Vec<MailOp>,
    label: String,
    queue: State<'_, Arc<OpQueue>>,
    store: State<'_, Arc<dyn MailStore>>,
    config: State<'_, AppConfig>,
) -> Result<(), String> {
    if ops.is_empty() {
        return Ok(());
    }
    queue_items(&app, ops, label, &queue, &store, config.undo_send_seconds).await
}

async fn queue_items(
    app: &AppHandle,
    ops: Vec<MailOp>,
    label: String,
    queue: &Arc<OpQueue>,
    store: &Arc<dyn MailStore>,
    undo_seconds: u32,
) -> Result<(), String> {
    // Snapshot + optimistically apply each op to the store, so a repaint
    // during the undo window already shows the change.
    let mut items: Vec<(MailOp, Message)> = Vec::with_capacity(ops.len());
    for op in ops {
        let id = op.message_id().to_string();
        let snapshot = store_call(store, move |s| s.get_message(&id)).await?;
        let Some(snapshot) = snapshot else { continue };
        apply_op_locally(store, &op).await?;
        items.push((op, snapshot));
    }
    if items.is_empty() {
        return Err("no messages found".to_string());
    }

    let count = items.len();
    let subject = if count == 1 {
        items[0].1.summary.subject.clone()
    } else {
        format!("{count} messages")
    };

    let id = queue
        .next_id
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let delay = undo_seconds.min(120);
    let _ = app.emit(
        "op:queued",
        &OpQueuedEvent {
            id,
            label,
            subject,
            deadline_ms: now_ms() + u64::from(delay) * 1000,
        },
    );
    queue.pending.lock().unwrap().insert(id, items);

    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        if delay > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(u64::from(delay))).await;
        }
        perform_op(&handle, id).await;
    });
    Ok(())
}

/// Cancel a queued action and restore every optimistic local change.
#[tauri::command]
async fn undo_op(
    id: u64,
    queue: State<'_, Arc<OpQueue>>,
    store: State<'_, Arc<dyn MailStore>>,
) -> Result<(), String> {
    let entry = queue.pending.lock().unwrap().remove(&id);
    let Some(items) = entry else {
        return Err("too late — already applied".to_string());
    };
    let snapshots: Vec<Message> = items.into_iter().map(|(_, m)| m).collect();
    store_call(&store, move |s| s.upsert_messages(&snapshots)).await
}

/// Apply one op's local mutation to the store (optimistic).
async fn apply_op_locally(store: &Arc<dyn MailStore>, op: &MailOp) -> Result<(), String> {
    let op = op.clone();
    store_call(store, move |s| match &op {
        MailOp::SetRead { id, is_read } => s.set_read_state(id, *is_read),
        MailOp::SetFlag { id, flagged } => {
            s.set_flag_status(id, if *flagged { "flagged" } else { "notFlagged" })
        }
        MailOp::SetCategories { id, categories } => s.set_categories(id, categories),
        MailOp::SetInference { id, focused } => {
            s.set_inference(id, if *focused { "focused" } else { "other" })
        }
        MailOp::Move { id, dest_folder_id } => s.move_message(id, dest_folder_id, None),
        MailOp::HardDelete { id } => s.delete_message(id),
    })
    .await
}

/// On lapse, send the whole action as one (chunked) `$batch` and reconcile
/// per item. Claim-by-remove: a missing entry was undone → no chunk fires.
async fn perform_op(app: &AppHandle, id: u64) {
    let Some(items) = app
        .state::<Arc<OpQueue>>()
        .pending
        .lock()
        .unwrap()
        .remove(&id)
    else {
        return;
    };
    let store = Arc::clone(&app.state::<Arc<dyn MailStore>>());
    let writer = GraphMailSource::new(Arc::clone(&app.state::<Arc<Authenticator>>()));

    let total = items.len();
    let outcome = execute_batch(&store, &writer, items).await;
    let failed = outcome.failed_count();
    let error = if failed == 0 {
        None
    } else {
        tracing::warn!(
            failed,
            total,
            "bulk op partially failed; failures rolled back"
        );
        Some(format!("{failed} of {total} couldn't be completed"))
    };
    let _ = app.emit("op:done", &SendResultEvent { id, error });
}

// --- Drafts (createReply/createForward/blank) ---

#[tauri::command]
async fn create_reply_draft(
    mode: String,
    message_id: String,
    auth: State<'_, Arc<Authenticator>>,
) -> Result<DraftHandle, String> {
    GraphMailSource::new(Arc::clone(&auth))
        .create_reply_draft(&message_id, &mode)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn create_blank_draft(auth: State<'_, Arc<Authenticator>>) -> Result<DraftHandle, String> {
    GraphMailSource::new(Arc::clone(&auth))
        .create_draft()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn autosave_draft(
    draft_id: String,
    subject: String,
    body: String,
    to: Vec<EmailAddress>,
    cc: Vec<EmailAddress>,
    bcc: Vec<EmailAddress>,
    auth: State<'_, Arc<Authenticator>>,
) -> Result<(), String> {
    GraphMailSource::new(Arc::clone(&auth))
        .update_draft(&draft_id, &subject, &body, &to, &cc, &bcc)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn send_draft(draft_id: String, auth: State<'_, Arc<Authenticator>>) -> Result<(), String> {
    GraphMailSource::new(Arc::clone(&auth))
        .send_draft(&draft_id)
        .await
        .map_err(|e| e.to_string())
}

/// Attach a base64 file to a draft — inline if small, chunked upload session
/// if >3 MB, with `attach:progress` events carrying (sent, total).
#[tauri::command]
async fn attach_to_draft(
    app: AppHandle,
    draft_id: String,
    name: String,
    content_type: String,
    content_base64: String,
    auth: State<'_, Arc<Authenticator>>,
) -> Result<(), String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(content_base64.as_bytes())
        .map_err(|_| "attachment is not valid base64".to_string())?;
    let handle = app.clone();
    let name_for_evt = name.clone();
    GraphMailSource::new(Arc::clone(&auth))
        .attach_to_draft(
            &draft_id,
            &name,
            &content_type,
            &bytes,
            move |sent, total| {
                let _ = handle.emit(
                    "attach:progress",
                    &serde_json::json!({ "name": name_for_evt, "sent": sent, "total": total }),
                );
            },
        )
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn autocomplete_recipients(
    query: String,
    store: State<'_, Arc<dyn MailStore>>,
) -> Result<Vec<(String, String)>, String> {
    if query.trim().is_empty() {
        return Ok(vec![]);
    }
    store_call(&store, move |s| s.search_recipients(query.trim(), 8)).await
}

#[tauri::command]
async fn get_signature(
    kind: String,
    auth: State<'_, Arc<Authenticator>>,
    store: State<'_, Arc<dyn MailStore>>,
) -> Result<Option<String>, String> {
    let account = auth
        .current_account()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "not signed in".to_string())?;
    store_call(&store, move |s| s.get_signature(&account.upn, &kind)).await
}

#[tauri::command]
async fn set_signature(
    kind: String,
    content: String,
    auth: State<'_, Arc<Authenticator>>,
    store: State<'_, Arc<dyn MailStore>>,
) -> Result<(), String> {
    let account = auth
        .current_account()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "not signed in".to_string())?;
    store_call(&store, move |s| {
        s.set_signature(&account.upn, &kind, &content)
    })
    .await
}

// --- Phase 8A: mailbox settings (out-of-office, master categories, working
// hours). Account-level singular config, so these are plain load/save command
// pairs modeled on get_signature/set_signature — not the per-message OpQueue.
// Reads cache to the local `settings` KV table so the pane and category picker
// still render offline. ---

fn signed_in_upn(auth: &Authenticator) -> Result<String, String> {
    Ok(auth
        .current_account()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "not signed in".to_string())?
        .upn)
}

/// Re-list the master categories from Graph and refresh the local cache.
/// Shared by list/create/recolor/delete so every mutation returns the fresh
/// authoritative list and the cache stays coherent.
async fn list_and_cache_categories(
    auth: &Arc<Authenticator>,
    store: &Arc<dyn MailStore>,
    upn: &str,
) -> Result<Vec<MasterCategory>, String> {
    let source = GraphMailSource::new(Arc::clone(auth));
    let cats = source
        .list_master_categories()
        .await
        .map_err(|e| e.to_string())?;
    if let Ok(json) = serde_json::to_string(&cats) {
        let (acct, json) = (upn.to_string(), json);
        let _ = store_call(store, move |s| {
            s.set_setting(&acct, "master_categories", &json)
        })
        .await;
    }
    Ok(cats)
}

#[tauri::command]
async fn get_mailbox_settings(
    auth: State<'_, Arc<Authenticator>>,
    store: State<'_, Arc<dyn MailStore>>,
) -> Result<MailboxSettings, String> {
    let upn = signed_in_upn(&auth)?;
    let source = GraphMailSource::new(Arc::clone(&auth));
    match source.get_mailbox_settings().await {
        Ok(s) => {
            let days = s
                .working_hours
                .as_ref()
                .map(|w| w.days_of_week.len())
                .unwrap_or(0);
            tracing::info!(
                time_zone = %s.time_zone,
                date_format = %s.date_format,
                time_format = %s.time_format,
                working_days = days,
                "mailbox settings read from Graph"
            );
            if let Ok(json) = serde_json::to_string(&s) {
                let (acct, json) = (upn.clone(), json);
                let _ = store_call(&store, move |st| {
                    st.set_setting(&acct, "mailbox_settings", &json)
                })
                .await;
            }
            Ok(s)
        }
        // Offline (or a transient error): fall back to the cached blob so the
        // pane still opens. Only surface the error if nothing is cached.
        Err(e) => {
            let acct = upn.clone();
            let cached =
                store_call(&store, move |st| st.get_setting(&acct, "mailbox_settings")).await?;
            match cached.and_then(|j| serde_json::from_str::<MailboxSettings>(&j).ok()) {
                Some(s) => Ok(s),
                None => Err(e.to_string()),
            }
        }
    }
}

#[tauri::command]
async fn set_automatic_replies(
    replies: AutomaticReplies,
    auth: State<'_, Arc<Authenticator>>,
    store: State<'_, Arc<dyn MailStore>>,
) -> Result<(), String> {
    let upn = signed_in_upn(&auth)?;
    let source = GraphMailSource::new(Arc::clone(&auth));
    // Graph sanitizes the HTML bodies before the PATCH; on failure the caller
    // rolls back optimistic UI state and surfaces the error.
    source
        .patch_automatic_replies(&replies)
        .await
        .map_err(|e| e.to_string())?;
    // Keep the cached settings blob coherent with what we just committed
    // (store the sanitized form the server received).
    let acct = upn.clone();
    let cached = store_call(&store, move |st| st.get_setting(&acct, "mailbox_settings")).await?;
    let mut settings = cached
        .and_then(|j| serde_json::from_str::<MailboxSettings>(&j).ok())
        .unwrap_or_default();
    settings.automatic_replies_setting = replies.sanitized();
    if let Ok(json) = serde_json::to_string(&settings) {
        let (acct, json) = (upn, json);
        let _ = store_call(&store, move |st| {
            st.set_setting(&acct, "mailbox_settings", &json)
        })
        .await;
    }
    Ok(())
}

#[tauri::command]
async fn list_master_categories(
    auth: State<'_, Arc<Authenticator>>,
    store: State<'_, Arc<dyn MailStore>>,
) -> Result<Vec<MasterCategory>, String> {
    let upn = signed_in_upn(&auth)?;
    match list_and_cache_categories(&auth, &store, &upn).await {
        Ok(cats) => Ok(cats),
        // Offline: serve the cached list rather than an empty picker.
        Err(e) => {
            let acct = upn.clone();
            let cached =
                store_call(&store, move |st| st.get_setting(&acct, "master_categories")).await?;
            match cached.and_then(|j| serde_json::from_str::<Vec<MasterCategory>>(&j).ok()) {
                Some(cats) => Ok(cats),
                None => Err(e),
            }
        }
    }
}

#[tauri::command]
async fn create_master_category(
    display_name: String,
    color: String,
    auth: State<'_, Arc<Authenticator>>,
    store: State<'_, Arc<dyn MailStore>>,
) -> Result<Vec<MasterCategory>, String> {
    if !is_offered_preset(&color) {
        return Err(format!("unsupported category color: {color}"));
    }
    let upn = signed_in_upn(&auth)?;
    let source = GraphMailSource::new(Arc::clone(&auth));
    source
        .create_master_category(&display_name, &color)
        .await
        .map_err(|e| e.to_string())?;
    list_and_cache_categories(&auth, &store, &upn).await
}

#[tauri::command]
async fn set_master_category_color(
    id: String,
    color: String,
    auth: State<'_, Arc<Authenticator>>,
    store: State<'_, Arc<dyn MailStore>>,
) -> Result<Vec<MasterCategory>, String> {
    if !is_offered_preset(&color) {
        return Err(format!("unsupported category color: {color}"));
    }
    let upn = signed_in_upn(&auth)?;
    let source = GraphMailSource::new(Arc::clone(&auth));
    source
        .set_master_category_color(&id, &color)
        .await
        .map_err(|e| e.to_string())?;
    list_and_cache_categories(&auth, &store, &upn).await
}

#[tauri::command]
async fn delete_master_category(
    id: String,
    auth: State<'_, Arc<Authenticator>>,
    store: State<'_, Arc<dyn MailStore>>,
) -> Result<Vec<MasterCategory>, String> {
    let upn = signed_in_upn(&auth)?;
    let source = GraphMailSource::new(Arc::clone(&auth));
    source
        .delete_master_category(&id)
        .await
        .map_err(|e| e.to_string())?;
    list_and_cache_categories(&auth, &store, &upn).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // WebKitGTK's accelerated compositing crashes the WebKitWebProcess on
    // some Linux driver/compositor combinations. Must be set before the
    // first webview is created; an explicit user override wins.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_COMPOSITING_MODE").is_none() {
        std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    tauri::Builder::default()
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&config_dir)?;
            std::fs::create_dir_all(&data_dir)?;
            let config = AppConfig::load(&config_dir);

            let db_path = config
                .db_path
                .clone()
                .unwrap_or_else(|| data_dir.join("spite.db"));
            let mail_store: Arc<dyn MailStore> = Arc::new(SqliteMailStore::open(db_path)?);
            app.manage(mail_store);

            let auth = Arc::new(Authenticator::new(
                config.clone(),
                Arc::new(KeyringTokenStore),
                data_dir,
            ));
            app.manage(auth);
            app.manage(config);
            app.manage(ComposeRegistry::default());
            app.manage(Arc::new(SendQueue::default()));
            app.manage(Arc::new(OpQueue::default()));
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the main window must not orphan compose windows (the
            // app only exits when every window is gone). Graceful close so
            // a dirty composer's discard-draft guard can still intervene —
            // if the user keeps a draft open, the app deliberately stays
            // alive rather than dropping the content.
            if window.label() == "main"
                && matches!(event, tauri::WindowEvent::CloseRequested { .. })
            {
                for (label, compose) in window.app_handle().webview_windows() {
                    if label.starts_with("compose-") {
                        let _ = compose.close();
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            sign_in,
            silent_sign_in,
            sign_out,
            list_folders,
            list_messages,
            unread_counts,
            get_message,
            fetch_message_body,
            sync_folders,
            sync_folder,
            get_sync_status,
            get_sender_pref,
            set_sender_pref,
            open_compose,
            get_compose_context,
            queue_send,
            undo_send,
            autocomplete_recipients,
            get_signature,
            set_signature,
            search_local,
            search_server,
            list_saved_searches,
            save_search,
            delete_saved_search,
            get_keymap,
            get_auto_read_dwell,
            repair_summaries,
            get_mailbox_settings,
            set_automatic_replies,
            list_master_categories,
            create_master_category,
            set_master_category_color,
            delete_master_category,
            apply_op,
            queue_op,
            queue_bulk_op,
            undo_op,
            create_reply_draft,
            create_blank_draft,
            autosave_draft,
            send_draft,
            attach_to_draft
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            // Quitting mid-countdown must not drop queued mail: the user
            // pressed Send, so fire the pending sends now instead of never.
            // (Bounded by the HTTP client's 30s timeout per message.)
            if let tauri::RunEvent::ExitRequested { .. } = event {
                let send_ids: Vec<u64> = app_handle
                    .state::<Arc<SendQueue>>()
                    .pending
                    .lock()
                    .unwrap()
                    .keys()
                    .copied()
                    .collect();
                // Pending management ops committed to the server too — flush
                // them so an archive/delete the user asked for isn't lost.
                let op_ids: Vec<u64> = app_handle
                    .state::<Arc<OpQueue>>()
                    .pending
                    .lock()
                    .unwrap()
                    .keys()
                    .copied()
                    .collect();
                if !send_ids.is_empty() || !op_ids.is_empty() {
                    tracing::info!(
                        sends = send_ids.len(),
                        ops = op_ids.len(),
                        "flushing queued work before exit"
                    );
                    let handle = app_handle.clone();
                    tauri::async_runtime::block_on(async move {
                        for id in send_ids {
                            perform_send(&handle, id).await;
                        }
                        for id in op_ids {
                            perform_op(&handle, id).await;
                        }
                    });
                }
            }
        });
}
