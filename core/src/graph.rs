//! Minimal Microsoft Graph client. Phase 1 only needs `/me`; mail endpoints
//! arrive with sync in Phase 3.

use serde::Deserialize;

pub const GRAPH_BASE: &str = "https://graph.microsoft.com/v1.0";

#[derive(Debug, thiserror::Error)]
pub enum GraphError {
    #[error("graph request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("graph returned {status}: {body}")]
    Status { status: u16, body: String },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Me {
    /// Can be null for some account types; callers fall back to the UPN.
    pub display_name: Option<String>,
    pub user_principal_name: String,
}

pub async fn get_me(http: &reqwest::Client, access_token: &str) -> Result<Me, GraphError> {
    let resp = http
        .get(format!("{GRAPH_BASE}/me"))
        .bearer_auth(access_token)
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(GraphError::Status {
            status: resp.status().as_u16(),
            body: resp.text().await.unwrap_or_default(),
        });
    }
    Ok(resp.json().await?)
}
