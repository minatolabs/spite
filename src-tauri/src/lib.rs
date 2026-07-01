use std::sync::Arc;

use spite_core::auth::token_store::KeyringTokenStore;
use spite_core::auth::{Account, Authenticator, DeviceCodePrompt};
use spite_core::config::AppConfig;
use tauri::{AppHandle, Emitter, Manager, State};

#[tauri::command]
async fn sign_in(app: AppHandle, auth: State<'_, Authenticator>) -> Result<Account, String> {
    auth.sign_in(|prompt: DeviceCodePrompt| {
        let _ = app.emit("auth:device-code", &prompt);
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn silent_sign_in(auth: State<'_, Authenticator>) -> Result<Account, String> {
    auth.silent_sign_in().await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn sign_out(auth: State<'_, Authenticator>) -> Result<(), String> {
    auth.sign_out().await.map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&config_dir)?;
            std::fs::create_dir_all(&data_dir)?;
            let config = AppConfig::load(&config_dir);
            let auth = Authenticator::new(config, Arc::new(KeyringTokenStore), data_dir);
            app.manage(auth);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![sign_in, silent_sign_in, sign_out])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
