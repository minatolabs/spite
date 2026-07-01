//! App configuration: Entra `client_id` + authority. Config-file overridable
//! so users can bring their own app registration (or point authority at a
//! single tenant for testing) without a code change.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Shipped client_id. Empty until Spite's own multitenant Entra app is
/// registered; until then users must set `client_id` in `config.json`.
pub const DEFAULT_CLIENT_ID: &str = "";
/// `/common` resolves each user's tenant from their sign-in address.
pub const DEFAULT_AUTHORITY: &str = "https://login.microsoftonline.com/common";
pub const CONFIG_FILE: &str = "config.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub client_id: String,
    pub authority: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            client_id: DEFAULT_CLIENT_ID.to_string(),
            authority: DEFAULT_AUTHORITY.to_string(),
        }
    }
}

/// Partial shape of `config.json`; absent fields keep their defaults.
#[derive(Debug, Default, Deserialize)]
struct ConfigOverrides {
    client_id: Option<String>,
    authority: Option<String>,
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
}
