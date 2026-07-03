//! Microsoft Graph client: `/me` (Phase 1) plus the mail/delta surface
//! (`GraphMailSource`, Phase 3). Full-body fetch arrives in Phase 4.

use std::sync::Arc;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::auth::{AuthError, Authenticator};
use crate::store::{Folder, Message, MessageSummary};
use crate::sync::{DeltaPage, DeltaRequest, MailSource, PageToken, SourceError};

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

/// Summary fields synced into the local store. Bodies are deliberately
/// excluded (lazy-loaded on open, Phase 4).
pub const MESSAGE_SELECT: &str =
    "id,subject,from,receivedDateTime,bodyPreview,isRead,hasAttachments,conversationId,parentFolderId";
const DELTA_PAGE_SIZE: u32 = 100;
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// Real `MailSource` backed by Microsoft Graph.
pub struct GraphMailSource {
    auth: Arc<Authenticator>,
    http: reqwest::Client,
}

impl GraphMailSource {
    pub fn new(auth: Arc<Authenticator>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .expect("reqwest client construction cannot fail with these options");
        Self { auth, http }
    }

    /// Authenticated GET with Graph error mapping: 401 → one forced token
    /// refresh + retry; 429 → `Throttled` carrying Retry-After (the sync
    /// orchestrator does the actual backing off).
    async fn get_json<T: DeserializeOwned>(
        &self,
        url: &str,
        query: &[(&str, String)],
        page_size: Option<u32>,
    ) -> Result<T, SourceError> {
        for attempt in 0..2 {
            let token = self.auth.access_token().await.map_err(|e| match e {
                AuthError::NeedsSignIn | AuthError::NotConfigured => SourceError::Unauthorized,
                other => SourceError::Http(other.to_string()),
            })?;

            let mut req = self.http.get(url).bearer_auth(token);
            if let Some(size) = page_size {
                req = req.header("Prefer", format!("odata.maxpagesize={size}"));
            }
            if !query.is_empty() {
                req = req.query(query);
            }
            let resp = req
                .send()
                .await
                .map_err(|e| SourceError::Http(e.to_string()))?;

            let status = resp.status().as_u16();
            match status {
                401 if attempt == 0 => {
                    self.auth.invalidate_session().await;
                    continue;
                }
                401 => return Err(SourceError::Unauthorized),
                429 => {
                    let retry_after = resp
                        .headers()
                        .get("Retry-After")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|s| s.parse::<u64>().ok())
                        .unwrap_or(5);
                    return Err(SourceError::Throttled {
                        retry_after: Duration::from_secs(retry_after),
                    });
                }
                s if !(200..300).contains(&s) => {
                    return Err(SourceError::Http(format!(
                        "{s}: {}",
                        resp.text().await.unwrap_or_default()
                    )));
                }
                _ => {
                    return resp
                        .json()
                        .await
                        .map_err(|e| SourceError::Protocol(e.to_string()));
                }
            }
        }
        unreachable!("the 401-retry loop always returns within two attempts")
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphFolder {
    id: String,
    display_name: Option<String>,
    parent_folder_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GraphRecipient {
    #[serde(rename = "emailAddress")]
    email_address: Option<GraphEmailAddress>,
}

#[derive(Debug, Deserialize)]
struct GraphEmailAddress {
    name: Option<String>,
    address: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphMessage {
    id: String,
    /// Present (with reason `deleted` or `changed`) when the message left
    /// the folder — either kind maps to a local delete.
    #[serde(rename = "@removed")]
    removed: Option<serde_json::Value>,
    subject: Option<String>,
    from: Option<GraphRecipient>,
    received_date_time: Option<String>,
    body_preview: Option<String>,
    is_read: Option<bool>,
    has_attachments: Option<bool>,
    conversation_id: Option<String>,
    parent_folder_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DeltaResponse {
    value: Vec<GraphMessage>,
    #[serde(rename = "@odata.nextLink")]
    next_link: Option<String>,
    #[serde(rename = "@odata.deltaLink")]
    delta_link: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ListResponse {
    value: Vec<GraphMessage>,
}

fn parse_epoch(rfc3339: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .ok()
        .map(|dt| dt.timestamp())
}

fn epoch_to_rfc3339(epoch: i64) -> String {
    chrono::DateTime::from_timestamp(epoch, 0)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn to_domain(g: GraphMessage) -> Message {
    let (from_name, from_address) = match g.from.and_then(|r| r.email_address) {
        Some(e) => (e.name.unwrap_or_default(), e.address.unwrap_or_default()),
        None => (String::new(), String::new()), // e.g. drafts have no `from`
    };
    Message {
        summary: MessageSummary {
            id: g.id,
            // Overridden by the sync orchestrator, which pins rows to the
            // folder being synced.
            folder_id: g.parent_folder_id.unwrap_or_default(),
            subject: g.subject.unwrap_or_default(),
            from_name,
            from_address,
            received_at: g
                .received_date_time
                .as_deref()
                .and_then(parse_epoch)
                .unwrap_or(0),
            preview: g.body_preview.unwrap_or_default(),
            is_read: g.is_read.unwrap_or(false),
            has_attachments: g.has_attachments.unwrap_or(false),
        },
        conversation_id: g.conversation_id,
        body_html: None, // never fetched during sync; lazy-loaded in Phase 4
    }
}

fn split_delta(items: Vec<GraphMessage>) -> (Vec<Message>, Vec<String>) {
    let mut messages = Vec::new();
    let mut removed = Vec::new();
    for item in items {
        if item.removed.is_some() {
            removed.push(item.id);
        } else {
            messages.push(to_domain(item));
        }
    }
    (messages, removed)
}

impl MailSource for GraphMailSource {
    async fn get_inbox_folder(&self) -> Result<Folder, SourceError> {
        let f: GraphFolder = self
            .get_json(
                &format!("{GRAPH_BASE}/me/mailFolders/inbox"),
                &[("$select", "id,displayName,parentFolderId".to_string())],
                None,
            )
            .await?;
        Ok(Folder {
            id: f.id,
            display_name: f.display_name.unwrap_or_else(|| "Inbox".to_string()),
            // v1.0 mailFolder has no wellKnownName property; we addressed the
            // folder by its well-known alias, so set it ourselves.
            well_known_name: Some("inbox".to_string()),
            parent_id: f.parent_folder_id,
        })
    }

    async fn backfill_cutoff(&self, folder_id: &str, n: u32) -> Result<i64, SourceError> {
        let n = n.clamp(1, 1000);
        let resp: ListResponse = self
            .get_json(
                &format!("{GRAPH_BASE}/me/mailFolders/{folder_id}/messages"),
                &[
                    ("$top", n.to_string()),
                    ("$orderby", "receivedDateTime desc".to_string()),
                    ("$select", "id,receivedDateTime".to_string()),
                ],
                Some(n),
            )
            .await?;
        let oldest = resp
            .value
            .iter()
            .filter_map(|m| m.received_date_time.as_deref())
            .filter_map(parse_epoch)
            .min();
        Ok(oldest.unwrap_or_else(crate::sync::now_epoch))
    }

    async fn fetch_delta_page(&self, request: &DeltaRequest) -> Result<DeltaPage, SourceError> {
        let resp: DeltaResponse = match request {
            DeltaRequest::Initial {
                folder_id,
                since_epoch,
            } => {
                // Query options are set here once; Graph encodes them into
                // the next/delta tokens, so follow-up URLs go out verbatim.
                self.get_json(
                    &format!("{GRAPH_BASE}/me/mailFolders/{folder_id}/messages/delta"),
                    &[
                        (
                            "$filter",
                            format!("receivedDateTime ge {}", epoch_to_rfc3339(*since_epoch)),
                        ),
                        ("$orderby", "receivedDateTime desc".to_string()),
                        ("$select", MESSAGE_SELECT.to_string()),
                    ],
                    Some(DELTA_PAGE_SIZE),
                )
                .await?
            }
            DeltaRequest::Url(url) => self.get_json(url, &[], Some(DELTA_PAGE_SIZE)).await?,
        };

        let token = match (resp.next_link, resp.delta_link) {
            (Some(next), _) => PageToken::Next(next),
            (None, Some(delta)) => PageToken::Delta(delta),
            (None, None) => {
                return Err(SourceError::Protocol(
                    "delta response carried neither nextLink nor deltaLink".to_string(),
                ));
            }
        };
        let (messages, removed_ids) = split_delta(resp.value);
        Ok(DeltaPage {
            messages,
            removed_ids,
            token,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_round_trip_and_parse() {
        assert_eq!(parse_epoch("2026-07-02T10:00:00Z"), Some(1_782_986_400));
        assert_eq!(epoch_to_rfc3339(1_782_986_400), "2026-07-02T10:00:00Z");
        assert_eq!(parse_epoch("not a date"), None);
    }

    #[test]
    fn delta_items_split_into_upserts_and_removals() {
        let items: Vec<GraphMessage> = serde_json::from_str(
            r#"[
                {"id": "keep", "subject": "hi", "isRead": true,
                 "receivedDateTime": "2026-07-02T10:00:00Z",
                 "from": {"emailAddress": {"name": "A", "address": "a@x.com"}}},
                {"id": "gone", "@removed": {"reason": "deleted"}},
                {"id": "moved", "@removed": {"reason": "changed"}}
            ]"#,
        )
        .unwrap();
        let (messages, removed) = split_delta(items);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].summary.id, "keep");
        assert_eq!(messages[0].summary.from_address, "a@x.com");
        assert!(messages[0].summary.is_read);
        assert_eq!(removed, ["gone", "moved"]);
    }

    #[test]
    fn null_from_maps_to_empty_sender() {
        let item: GraphMessage =
            serde_json::from_str(r#"{"id": "draft", "subject": null}"#).unwrap();
        let m = to_domain(item);
        assert_eq!(m.summary.from_name, "");
        assert_eq!(m.summary.from_address, "");
        assert_eq!(m.summary.subject, "");
        assert_eq!(m.summary.received_at, 0);
        assert!(m.body_html.is_none());
    }
}
