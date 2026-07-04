use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use spite_core::auth::token_store::KeyringTokenStore;
use spite_core::auth::{Account, Authenticator, DeviceCodePrompt};
use spite_core::compose::{self, ComposeMode, Draft, EmailAddress};
use spite_core::config::AppConfig;
use spite_core::graph::GraphMailSource;
use spite_core::sanitize::sanitize_html;
use spite_core::store::{
    Folder, MailStore, MailStoreError, Message, MessageSummary, SqliteMailStore, SyncState,
};
use spite_core::sync::{sync_folder as run_sync_folder, SyncReport};
use tauri::{AppHandle, Emitter, Manager, State};

/// Pending compose-window parameters, keyed by window label. Avoids pushing
/// Graph message ids through URL query encoding.
#[derive(Default)]
struct ComposeRegistry(Mutex<HashMap<String, (String, Option<String>)>>);

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
    let cached = {
        let id = id.clone();
        store_call(&store, move |s| s.get_message(&id)).await?
    }
    .ok_or_else(|| "message not found".to_string())?;

    if let Some(body) = cached.body_html {
        return Ok(MessageBody {
            body,
            content_type: cached
                .body_content_type
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

    {
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
    registry: State<'_, ComposeRegistry>,
    mode: String,
    message_id: Option<String>,
) -> Result<(), String> {
    let label = format!(
        "compose-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_millis()
    );
    registry
        .0
        .lock()
        .unwrap()
        .insert(label.clone(), (mode, message_id));
    tauri::WebviewWindowBuilder::new(
        &app,
        &label,
        tauri::WebviewUrl::App(format!("index.html?compose=1&label={label}").into()),
    )
    .title("Compose — Spite")
    .inner_size(760.0, 680.0)
    .build()
    .map_err(|e| e.to_string())?;
    Ok(())
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
}

#[tauri::command]
async fn get_compose_context(
    label: String,
    registry: State<'_, ComposeRegistry>,
    auth: State<'_, Arc<Authenticator>>,
    store: State<'_, Arc<dyn MailStore>>,
) -> Result<ComposeContext, String> {
    let (mode_str, message_id) = registry
        .0
        .lock()
        .unwrap()
        .get(&label)
        .cloned()
        .ok_or_else(|| "unknown compose window".to_string())?;
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
        return Ok(ComposeContext {
            mode: mode_str,
            to: vec![],
            cc: vec![],
            subject: String::new(),
            quoted_html: None,
            in_reply_to: None,
            references: vec![],
            signature,
            degraded: false,
        });
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
        mode: mode_str,
        to,
        cc,
        subject,
        quoted_html,
        in_reply_to,
        references,
        signature,
        degraded,
    })
}

/// Build MIME and send. On success, recipients feed the local autocomplete
/// history. On failure the error string returns to the composer, which keeps
/// the draft — content is never lost here.
#[tauri::command]
async fn send_mail(
    draft: Draft,
    auth: State<'_, Arc<Authenticator>>,
    store: State<'_, Arc<dyn MailStore>>,
) -> Result<(), String> {
    let account = auth
        .current_account()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "not signed in".to_string())?;
    let from = EmailAddress {
        name: account.display_name,
        address: account.upn,
    };
    let mime = compose::build_mime(&draft, &from).map_err(|e| e.to_string())?;

    let source = GraphMailSource::new(Arc::clone(&auth));
    source
        .send_mime(&mime)
        .await
        .map_err(|e| format!("send failed: {e}"))?;

    let recipients: Vec<(String, String)> = draft
        .to
        .iter()
        .chain(&draft.cc)
        .chain(&draft.bcc)
        .map(|a| (a.address.clone(), a.name.clone()))
        .collect();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // History is best-effort; the mail is already sent.
    let _ = store_call(&store, move |s| s.record_recipients(&recipients, now)).await;
    Ok(())
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
            Ok(())
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
            send_mail,
            autocomplete_recipients,
            get_signature,
            set_signature
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
