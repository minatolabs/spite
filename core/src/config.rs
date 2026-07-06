//! App configuration: Entra `client_id` + authority. Config-file overridable
//! so users can bring their own app registration (or point authority at a
//! single tenant for testing) without a code change.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Shipped client_id. Empty until Spite's own multitenant Entra app is
/// registered; until then users must set `client_id` in `config.json`.
pub const DEFAULT_CLIENT_ID: &str = "";
/// `/common` resolves each user's tenant from their sign-in address.
pub const DEFAULT_AUTHORITY: &str = "https://login.microsoftonline.com/common";
pub const CONFIG_FILE: &str = "config.json";
/// Initial-sync window size: how many recent messages to backfill.
pub const DEFAULT_BACKFILL_COUNT: u32 = 200;
/// Undo-send window: seconds a queued message waits before actually sending.
pub const DEFAULT_UNDO_SEND_SECONDS: u32 = 15;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub client_id: String,
    pub authority: String,
    /// Mail database location; `None` means the platform app-data dir.
    pub db_path: Option<PathBuf>,
    /// Recent messages to backfill on first sync (clamped to 1..=1000).
    pub backfill_count: u32,
    /// Undo-send delay in seconds (clamped to 0..=120; 0 sends immediately).
    pub undo_send_seconds: u32,
    /// Keyboard-shortcut overrides (action name → key); unlisted actions
    /// keep the vim-flavored defaults defined in the UI.
    pub keymap: HashMap<String, String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            client_id: DEFAULT_CLIENT_ID.to_string(),
            authority: DEFAULT_AUTHORITY.to_string(),
            db_path: None,
            backfill_count: DEFAULT_BACKFILL_COUNT,
            undo_send_seconds: DEFAULT_UNDO_SEND_SECONDS,
            keymap: HashMap::new(),
        }
    }
}

/// Partial shape of `config.json`; absent fields keep their defaults.
#[derive(Debug, Default, Deserialize)]
struct ConfigOverrides {
    client_id: Option<String>,
    authority: Option<String>,
    db_path: Option<PathBuf>,
    backfill_count: Option<u32>,
    undo_send_seconds: Option<u32>,
    keymap: Option<HashMap<String, String>>,
}

impl AppConfig {
    /// Shipped defaults overlaid with `config.json` from `config_dir`, if present.
    pub fn load(config_dir: &Path) -> Self {
        let mut cfg = Self::default();
        let path = config_dir.join(CONFIG_FILE);
        match std::fs::read_to_string(&path) {
            Ok(raw) => match serde_json::from_str::<ConfigOverrides>(&raw) {
                Ok(overrides) => {
                    if let Some(v) = overrides.client_id {
                        cfg.client_id = v;
                    }
                    if let Some(v) = overrides.authority {
                        cfg.authority = v;
                    }
                    if let Some(v) = overrides.db_path {
                        cfg.db_path = Some(v);
                    }
                    if let Some(v) = overrides.backfill_count {
                        cfg.backfill_count = v;
                    }
                    if let Some(v) = overrides.undo_send_seconds {
                        cfg.undo_send_seconds = v;
                    }
                    if let Some(v) = overrides.keymap {
                        cfg.keymap = v;
                    }
                }
                Err(e) => eprintln!("spite: ignoring malformed {}: {e}", path.display()),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => eprintln!("spite: cannot read {}: {e}", path.display()),
        }
        cfg
    }

    /// False while the placeholder client_id is in effect.
    pub fn is_configured(&self) -> bool {
        !self.client_id.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_no_config_file() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = AppConfig::load(dir.path());
        assert_eq!(cfg.authority, DEFAULT_AUTHORITY);
        assert!(!cfg.is_configured());
    }

    #[test]
    fn config_file_overrides_defaults() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(CONFIG_FILE),
            r#"{ "client_id": "11111111-2222-3333-4444-555555555555" }"#,
        )
        .unwrap();
        let cfg = AppConfig::load(dir.path());
        assert_eq!(cfg.client_id, "11111111-2222-3333-4444-555555555555");
        assert_eq!(
            cfg.authority, DEFAULT_AUTHORITY,
            "unset fields keep defaults"
        );
        assert!(cfg.is_configured());
    }

    #[test]
    fn db_path_override() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(CONFIG_FILE),
            r#"{ "db_path": "/tmp/custom/spite.db" }"#,
        )
        .unwrap();
        let cfg = AppConfig::load(dir.path());
        assert_eq!(
            cfg.db_path.as_deref(),
            Some(Path::new("/tmp/custom/spite.db"))
        );
        assert_eq!(AppConfig::default().db_path, None);
    }

    #[test]
    fn backfill_count_default_and_override() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            AppConfig::load(dir.path()).backfill_count,
            DEFAULT_BACKFILL_COUNT
        );
        std::fs::write(dir.path().join(CONFIG_FILE), r#"{ "backfill_count": 50 }"#).unwrap();
        assert_eq!(AppConfig::load(dir.path()).backfill_count, 50);
    }

    #[test]
    fn undo_send_seconds_default_and_override() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            AppConfig::load(dir.path()).undo_send_seconds,
            DEFAULT_UNDO_SEND_SECONDS
        );
        std::fs::write(
            dir.path().join(CONFIG_FILE),
            r#"{ "undo_send_seconds": 30 }"#,
        )
        .unwrap();
        assert_eq!(AppConfig::load(dir.path()).undo_send_seconds, 30);
    }

    #[test]
    fn keymap_overrides_parse() {
        let dir = tempfile::tempdir().unwrap();
        assert!(AppConfig::load(dir.path()).keymap.is_empty());
        std::fs::write(
            dir.path().join(CONFIG_FILE),
            r#"{ "keymap": { "focusSearch": "s", "next": "n" } }"#,
        )
        .unwrap();
        let cfg = AppConfig::load(dir.path());
        assert_eq!(cfg.keymap.get("focusSearch").map(String::as_str), Some("s"));
        assert_eq!(cfg.keymap.get("next").map(String::as_str), Some("n"));
    }
}
