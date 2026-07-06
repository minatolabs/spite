use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use spite_core::auth::token_store::KeyringTokenStore;
use spite_core::auth::{Account, Authenticator, DeviceCodePrompt};
use spite_core::compose::{self, ComposeMode, Draft, EmailAddress};
use spite_core::config::AppConfig;
use spite_core::graph::{GraphMailSource, ServerHit};
use spite_core::sanitize::sanitize_html;
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

    Ok(ComposeContext {
        to,
        cc,
        subject,
        quoted_html,
        in_reply_to,
        references,
        degraded,
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
            get_keymap
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            // Quitting mid-countdown must not drop queued mail: the user
            // pressed Send, so fire the pending sends now instead of never.
            // (Bounded by the HTTP client's 30s timeout per message.)
            if let tauri::RunEvent::ExitRequested { .. } = event {
                let queue = app_handle.state::<Arc<SendQueue>>();
                let ids: Vec<u64> = queue.pending.lock().unwrap().keys().copied().collect();
                if !ids.is_empty() {
                    tracing::info!(count = ids.len(), "flushing queued sends before exit");
                    let handle = app_handle.clone();
                    tauri::async_runtime::block_on(async move {
                        for id in ids {
                            perform_send(&handle, id).await;
                        }
                    });
                }
            }
        });
}
