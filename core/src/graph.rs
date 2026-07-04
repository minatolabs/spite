//! Microsoft Graph client: `/me` (Phase 1) plus the mail/delta surface
//! (`GraphMailSource`, Phase 3). Full-body fetch arrives in Phase 4.

use std::sync::Arc;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Deserialize;

use base64::Engine;

use crate::auth::{AuthError, Authenticator};
use crate::compose::{parse_references, EmailAddress, ReplyContext};
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

#[derive(Debug, Deserialize)]
struct FolderListResponse {
    value: Vec<GraphFolder>,
}

#[derive(Debug, Deserialize)]
struct GraphHeader {
    name: String,
    value: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphReplyContext {
    internet_message_id: Option<String>,
    internet_message_headers: Option<Vec<GraphHeader>>,
    from: Option<GraphRecipient>,
    reply_to: Option<Vec<GraphRecipient>>,
    to_recipients: Option<Vec<GraphRecipient>>,
    cc_recipients: Option<Vec<GraphRecipient>>,
    subject: Option<String>,
    received_date_time: Option<String>,
}

fn rec_to_email(r: GraphRecipient) -> Option<EmailAddress> {
    let e = r.email_address?;
    Some(EmailAddress {
        name: e.name.unwrap_or_default(),
        address: e.address.unwrap_or_default(),
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphBody {
    content_type: Option<String>,
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphMessageWithBody {
    body: Option<GraphBody>,
    unique_body: Option<GraphBody>,
}

/// A lazily-fetched message body. `content` is RAW Graph output — callers
/// must run HTML through `crate::sanitize::sanitize_html` before storing
/// or rendering it.
#[derive(Debug)]
pub struct FetchedBody {
    pub content: String,
    /// "html" or "text".
    pub content_type: String,
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
        body_html: None, // never fetched during sync; lazy-loaded on open
        body_content_type: None,
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

/// Well-known folder aliases, in the pinned display order the UI uses.
pub const WELL_KNOWN_FOLDERS: [&str; 6] = [
    "inbox",
    "sentitems",
    "drafts",
    "archive",
    "junkemail",
    "deleteditems",
];

impl GraphMailSource {
    /// All mail folders: the well-known set resolved via their aliases
    /// (v1.0's mailFolder has no wellKnownName property, so addressing each
    /// alias is how we learn which id is which), plus the user's top-level
    /// folders. An alias missing from the mailbox (404) is skipped.
    pub async fn list_all_folders(&self) -> Result<Vec<Folder>, SourceError> {
        let select = ("$select", "id,displayName,parentFolderId".to_string());
        let mut folders = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for alias in WELL_KNOWN_FOLDERS {
            let fetched: Result<GraphFolder, SourceError> = self
                .get_json(
                    &format!("{GRAPH_BASE}/me/mailFolders/{alias}"),
                    std::slice::from_ref(&select),
                    None,
                )
                .await;
            match fetched {
                Ok(f) => {
                    seen.insert(f.id.clone());
                    folders.push(Folder {
                        id: f.id,
                        display_name: f.display_name.unwrap_or_else(|| alias.to_string()),
                        well_known_name: Some(alias.to_string()),
                        parent_id: f.parent_folder_id,
                    });
                }
                Err(SourceError::Http(msg)) if msg.starts_with("404") => continue,
                Err(e) => return Err(e),
            }
        }

        let resp: FolderListResponse = self
            .get_json(
                &format!("{GRAPH_BASE}/me/mailFolders"),
                &[("$top", "100".to_string()), select],
                None,
            )
            .await?;
        for f in resp.value {
            if seen.insert(f.id.clone()) {
                folders.push(Folder {
                    id: f.id,
                    display_name: f.display_name.unwrap_or_default(),
                    well_known_name: None,
                    parent_id: f.parent_folder_id,
                });
            }
        }
        Ok(folders)
    }

    /// Send a complete RFC 5322 message. Graph accepts base64-encoded MIME
    /// on the same `sendMail` endpoint (Content-Type: text/plain) — the only
    /// documented way to control In-Reply-To/References, since the JSON
    /// payload restricts internetMessageHeaders to x-prefixed names.
    /// Saves to Sent Items by default. Requires only `Mail.Send`.
    pub async fn send_mime(&self, mime: &[u8]) -> Result<(), SourceError> {
        let encoded = base64::engine::general_purpose::STANDARD.encode(mime);
        for attempt in 0..2 {
            let token = self.auth.access_token().await.map_err(|e| match e {
                AuthError::NeedsSignIn | AuthError::NotConfigured => SourceError::Unauthorized,
                other => SourceError::Http(other.to_string()),
            })?;
            let resp = self
                .http
                .post(format!("{GRAPH_BASE}/me/sendMail"))
                .bearer_auth(token)
                .header("Content-Type", "text/plain")
                .body(encoded.clone())
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
                s if (200..300).contains(&s) => return Ok(()),
                s => {
                    return Err(SourceError::Http(format!(
                        "{s}: {}",
                        resp.text().await.unwrap_or_default()
                    )));
                }
            }
        }
        unreachable!("the 401-retry loop always returns within two attempts")
    }

    /// Everything reply/forward construction needs from the original
    /// message, including its internetMessageId and References chain.
    /// `Mail.Read` — no new scope.
    pub async fn fetch_reply_context(&self, id: &str) -> Result<ReplyContext, SourceError> {
        let g: GraphReplyContext = self
            .get_json(
                &format!("{GRAPH_BASE}/me/messages/{id}"),
                &[(
                    "$select",
                    "internetMessageId,internetMessageHeaders,from,replyTo,toRecipients,\
                     ccRecipients,subject,receivedDateTime"
                        .to_string(),
                )],
                None,
            )
            .await?;
        let references = g
            .internet_message_headers
            .unwrap_or_default()
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("References"))
            .map(|h| parse_references(&h.value))
            .unwrap_or_default();
        Ok(ReplyContext {
            internet_message_id: g.internet_message_id,
            references,
            from: g.from.and_then(rec_to_email),
            reply_to: g
                .reply_to
                .unwrap_or_default()
                .into_iter()
                .filter_map(rec_to_email)
                .collect(),
            to: g
                .to_recipients
                .unwrap_or_default()
                .into_iter()
                .filter_map(rec_to_email)
                .collect(),
            cc: g
                .cc_recipients
                .unwrap_or_default()
                .into_iter()
                .filter_map(rec_to_email)
                .collect(),
            subject: g.subject.unwrap_or_default(),
            received_at: g
                .received_date_time
                .as_deref()
                .and_then(parse_epoch)
                .unwrap_or(0),
        })
    }

    /// Lazy body fetch for a single message. Returns RAW content — the
    /// caller sanitizes HTML before it goes anywhere.
    pub async fn fetch_message_body(&self, id: &str) -> Result<FetchedBody, SourceError> {
        let resp: GraphMessageWithBody = self
            .get_json(
                &format!("{GRAPH_BASE}/me/messages/{id}"),
                &[("$select", "body,uniqueBody".to_string())],
                None,
            )
            .await?;
        let body = resp
            .body
            .or(resp.unique_body)
            .ok_or_else(|| SourceError::Protocol("message has no body".to_string()))?;
        let content_type = match body.content_type.as_deref() {
            Some("html") => "html",
            _ => "text",
        };
        Ok(FetchedBody {
            content: body.content.unwrap_or_default(),
            content_type: content_type.to_string(),
        })
    }
}

impl MailSource for GraphMailSource {
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
