use std::sync::Arc;

use serde::Serialize;
use spite_core::auth::token_store::KeyringTokenStore;
use spite_core::auth::{Account, Authenticator, DeviceCodePrompt};
use spite_core::config::AppConfig;
use spite_core::graph::GraphMailSource;
use spite_core::sanitize::sanitize_html;
use spite_core::store::{
    Folder, MailStore, MailStoreError, Message, MessageSummary, SqliteMailStore, SyncState,
};
use spite_core::sync::{sync_folder as run_sync_folder, SyncReport};
use tauri::{AppHandle, Emitter, Manager, State};

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
            set_sender_pref
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
