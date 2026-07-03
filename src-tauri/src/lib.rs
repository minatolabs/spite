use std::sync::Arc;

use spite_core::auth::token_store::KeyringTokenStore;
use spite_core::auth::{Account, Authenticator, DeviceCodePrompt};
use spite_core::config::AppConfig;
use spite_core::graph::GraphMailSource;
use spite_core::store::{MailStore, SqliteMailStore};
use spite_core::sync::{sync_inbox, SyncReport};
use tauri::{AppHandle, Emitter, Manager, State};

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
async fn sync_mailbox(
    auth: State<'_, Arc<Authenticator>>,
    store: State<'_, Arc<dyn MailStore>>,
    config: State<'_, AppConfig>,
) -> Result<SyncReport, String> {
    let source = GraphMailSource::new(Arc::clone(&auth));
    sync_inbox(&source, Arc::clone(&store), config.backfill_count)
        .await
        .map_err(|e| e.to_string())
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

            // PROVISIONAL (Phase 3): sync once at startup after silent
            // sign-in so acceptance can run without a UI. Phase 4 replaces
            // this with UI-driven sync.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let auth = Arc::clone(&handle.state::<Arc<Authenticator>>());
                let store = Arc::clone(&handle.state::<Arc<dyn MailStore>>());
                let backfill = handle.state::<AppConfig>().backfill_count;
                match auth.silent_sign_in().await {
                    Ok(account) => {
                        tracing::info!(upn = %account.upn, "signed in; starting startup sync");
                        let source = GraphMailSource::new(auth);
                        match sync_inbox(&source, store, backfill).await {
                            Ok(report) => tracing::info!(?report, "startup sync finished"),
                            Err(e) => tracing::warn!(error = %e, "startup sync failed"),
                        }
                    }
                    Err(e) => {
                        tracing::info!(reason = %e, "not signed in; skipping startup sync");
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            sign_in,
            silent_sign_in,
            sign_out,
            sync_mailbox
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
