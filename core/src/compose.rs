//! Outbound message construction: reply/forward semantics, RFC 5322
//! threading headers, quoting, and MIME building.
//!
//! Why MIME: Graph's JSON `sendMail` payload rejects standard headers in
//! `internetMessageHeaders` (custom names must start with `x-`), so
//! `In-Reply-To`/`References` can only be set by posting a complete
//! base64-encoded MIME message to the same endpoint — still `Mail.Send`.
//!
//! Everything here is pure and unit-tested; the network lives in `graph`.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use mail_builder::headers::address::Address;
use mail_builder::headers::message_id::MessageId;
use mail_builder::MessageBuilder;
use serde::{Deserialize, Serialize};

use crate::sanitize::sanitize_html;

/// Total raw attachment budget. Graph caps requests at ~4 MB and inline
/// attachments ride through two base64 passes (attachment→MIME, MIME→body),
/// a ~1.87× expansion — 2 MB raw stays safely under the limit. Anything
/// bigger needs upload sessions (`Mail.ReadWrite`, a later phase).
pub const MAX_ATTACHMENT_TOTAL_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailAddress {
    pub name: String,
    pub address: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ComposeMode {
    New,
    Reply,
    ReplyAll,
    Forward,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftAttachment {
    pub name: String,
    pub content_type: String,
    pub content_base64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Draft {
    pub to: Vec<EmailAddress>,
    #[serde(default)]
    pub cc: Vec<EmailAddress>,
    #[serde(default)]
    pub bcc: Vec<EmailAddress>,
    pub subject: String,
    pub body: String,
    /// "html" or "text".
    pub content_type: String,
    /// Normalized message-id (no angle brackets).
    #[serde(default)]
    pub in_reply_to: Option<String>,
    /// Normalized message-ids, oldest first.
    #[serde(default)]
    pub references: Vec<String>,
    #[serde(default)]
    pub attachments: Vec<DraftAttachment>,
}

/// What reply/forward construction needs from the original message.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ReplyContext {
    pub internet_message_id: Option<String>,
    /// The original's own References chain (normalized ids).
    pub references: Vec<String>,
    pub from: Option<EmailAddress>,
    pub reply_to: Vec<EmailAddress>,
    pub to: Vec<EmailAddress>,
    pub cc: Vec<EmailAddress>,
    pub subject: String,
    pub received_at: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum ComposeError {
    #[error("add at least one recipient")]
    NoRecipients,
    #[error(
        "attachments too large: {total} bytes exceeds the {max}-byte inline limit — \
         larger uploads arrive with a later phase"
    )]
    AttachmentsTooLarge { total: usize, max: usize },
    #[error("attachment {name:?} is not valid base64")]
    BadAttachment { name: String },
    #[error("could not build the message: {0}")]
    Mime(String),
}

/// Strip one layer of angle brackets + surrounding whitespace from a
/// message-id (`mail-builder` re-adds the brackets when writing headers).
pub fn normalize_msgid(raw: &str) -> String {
    raw.trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .to_string()
}

/// Split a raw `References` header into normalized ids.
pub fn parse_references(header: &str) -> Vec<String> {
    header
        .split_whitespace()
        .map(normalize_msgid)
        .filter(|s| !s.is_empty())
        .collect()
}

/// RFC 5322 §3.6.4: the reply's References = the original's References
/// followed by the original's Message-ID.
pub fn build_references(original_refs: &[String], internet_message_id: &str) -> Vec<String> {
    let mut refs: Vec<String> = original_refs
        .iter()
        .map(|r| normalize_msgid(r))
        .filter(|s| !s.is_empty())
        .collect();
    let mid = normalize_msgid(internet_message_id);
    if !mid.is_empty() && !refs.contains(&mid) {
        refs.push(mid);
    }
    refs
}

fn has_prefix(subject: &str, prefixes: &[&str]) -> bool {
    let s = subject.trim_start().to_lowercase();
    prefixes.iter().any(|p| s.starts_with(p))
}

/// `Re:`-prefix once; an existing Re:/RE: (any case) is left alone.
pub fn reply_subject(original: &str) -> String {
    if has_prefix(original, &["re:"]) {
        original.trim().to_string()
    } else {
        format!("Re: {}", original.trim())
    }
}

/// `Fw:`-prefix once; existing Fw:/Fwd: (any case) is left alone.
pub fn forward_subject(original: &str) -> String {
    if has_prefix(original, &["fw:", "fwd:"]) {
        original.trim().to_string()
    } else {
        format!("Fw: {}", original.trim())
    }
}

fn same_addr(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn dedupe(list: Vec<EmailAddress>, exclude: Option<&str>) -> Vec<EmailAddress> {
    let mut out: Vec<EmailAddress> = Vec::new();
    for item in list {
        if item.address.is_empty() {
            continue;
        }
        if let Some(ex) = exclude {
            if same_addr(&item.address, ex) {
                continue;
            }
        }
        if !out.iter().any(|o| same_addr(&o.address, &item.address)) {
            out.push(item);
        }
    }
    out
}

/// Recipient lists for a reply. Reply-all excludes the signed-in address
/// (case-insensitively) and drops nobody else; if excluding self would empty
/// `to` (replying to your own sent mail), the original `to` is kept.
pub fn reply_recipients(
    ctx: &ReplyContext,
    self_addr: &str,
    mode: ComposeMode,
) -> (Vec<EmailAddress>, Vec<EmailAddress>) {
    let originator: Vec<EmailAddress> = if !ctx.reply_to.is_empty() {
        ctx.reply_to.clone()
    } else {
        ctx.from.clone().into_iter().collect()
    };
    match mode {
        ComposeMode::Reply => {
            let to = dedupe(originator.clone(), Some(self_addr));
            if to.is_empty() {
                // Replying to yourself: target the original recipients.
                (dedupe(ctx.to.clone(), None), Vec::new())
            } else {
                (to, Vec::new())
            }
        }
        ComposeMode::ReplyAll => {
            let mut to = originator;
            to.extend(ctx.to.iter().cloned());
            let to = dedupe(to, Some(self_addr));
            let to = if to.is_empty() {
                dedupe(ctx.to.clone(), None)
            } else {
                to
            };
            let cc = dedupe(ctx.cc.clone(), Some(self_addr))
                .into_iter()
                .filter(|c| !to.iter().any(|t| same_addr(&t.address, &c.address)))
                .collect();
            (to, cc)
        }
        ComposeMode::New | ComposeMode::Forward => (Vec::new(), Vec::new()),
    }
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn attribution(from_name: &str, from_address: &str, received_at: i64) -> String {
    let date = chrono::DateTime::from_timestamp(received_at, 0)
        .map(|d| d.format("%a, %-d %b %Y at %H:%M").to_string())
        .unwrap_or_else(|| "an earlier date".to_string());
    let who = if from_name.is_empty() {
        from_address.to_string()
    } else {
        format!("{from_name} <{from_address}>")
    };
    format!("On {date}, {} wrote:", escape_html(&who))
}

/// Quoted HTML for reply/forward. The original body is re-sanitized here —
/// quoted content is attacker-controlled and gets no pass for being a quote.
pub fn quote_html(
    body_html: &str,
    from_name: &str,
    from_address: &str,
    received_at: i64,
) -> String {
    format!(
        "<p>{}</p><blockquote>{}</blockquote>",
        attribution(from_name, from_address, received_at),
        sanitize_html(body_html)
    )
}

/// Quoted plain text: attribution + `> `-prefixed lines.
pub fn quote_text(
    body_text: &str,
    from_name: &str,
    from_address: &str,
    received_at: i64,
) -> String {
    let quoted: String = body_text.lines().map(|l| format!("> {l}\n")).collect();
    let who = if from_name.is_empty() {
        from_address.to_string()
    } else {
        format!("{from_name} <{from_address}>")
    };
    let date = chrono::DateTime::from_timestamp(received_at, 0)
        .map(|d| d.format("%a, %-d %b %Y at %H:%M").to_string())
        .unwrap_or_else(|| "an earlier date".to_string());
    format!("On {date}, {who} wrote:\n{quoted}")
}

/// Decode + size-check attachments. Returns raw bytes per attachment.
pub fn validate_attachments(attachments: &[DraftAttachment]) -> Result<Vec<Vec<u8>>, ComposeError> {
    let mut decoded = Vec::with_capacity(attachments.len());
    let mut total = 0usize;
    for att in attachments {
        let bytes = BASE64.decode(att.content_base64.as_bytes()).map_err(|_| {
            ComposeError::BadAttachment {
                name: att.name.clone(),
            }
        })?;
        total += bytes.len();
        decoded.push(bytes);
    }
    if total > MAX_ATTACHMENT_TOTAL_BYTES {
        return Err(ComposeError::AttachmentsTooLarge {
            total,
            max: MAX_ATTACHMENT_TOTAL_BYTES,
        });
    }
    Ok(decoded)
}

/// Build the complete RFC 5322 message. Output goes to Graph `sendMail`
/// as base64 MIME (see `graph::GraphMailSource::send_mime`).
pub fn build_mime(draft: &Draft, from: &EmailAddress) -> Result<Vec<u8>, ComposeError> {
    if draft.to.is_empty() && draft.cc.is_empty() && draft.bcc.is_empty() {
        return Err(ComposeError::NoRecipients);
    }
    let attachment_bytes = validate_attachments(&draft.attachments)?;

    let mut builder = MessageBuilder::new()
        .from((from.name.as_str(), from.address.as_str()))
        .subject(draft.subject.as_str());
    if !draft.to.is_empty() {
        builder = builder.to(addr_list(&draft.to));
    }
    if !draft.cc.is_empty() {
        builder = builder.cc(addr_list(&draft.cc));
    }
    if !draft.bcc.is_empty() {
        builder = builder.bcc(addr_list(&draft.bcc));
    }
    if let Some(irt) = &draft.in_reply_to {
        let irt = normalize_msgid(irt);
        if !irt.is_empty() {
            builder = builder.in_reply_to(irt);
        }
    }
    if !draft.references.is_empty() {
        builder = builder.references(MessageId::new_list(
            draft.references.iter().map(|r| normalize_msgid(r)),
        ));
    }
    builder = if draft.content_type == "html" {
        builder.html_body(draft.body.as_str())
    } else {
        builder.text_body(draft.body.as_str())
    };
    for (att, bytes) in draft.attachments.iter().zip(attachment_bytes) {
        builder = builder.attachment(att.content_type.as_str(), att.name.as_str(), bytes);
    }
    builder
        .write_to_vec()
        .map_err(|e| ComposeError::Mime(e.to_string()))
}

fn addr_list(list: &[EmailAddress]) -> Address<'_> {
    Address::new_list(
        list.iter()
            .map(|a| Address::new_address(Some(a.name.as_str()), a.address.as_str()))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(name: &str, address: &str) -> EmailAddress {
        EmailAddress {
            name: name.into(),
            address: address.into(),
        }
    }

    fn ctx() -> ReplyContext {
        ReplyContext {
            internet_message_id: Some("<orig-123@mail.example>".into()),
            references: vec!["root-1@mail.example".into(), "mid-2@mail.example".into()],
            from: Some(addr("Alice", "alice@example.com")),
            reply_to: vec![],
            to: vec![addr("Me", "me@example.com"), addr("Bob", "bob@example.com")],
            cc: vec![
                addr("Carol", "carol@example.com"),
                addr("Me", "ME@example.com"),
            ],
            subject: "Quarterly numbers".into(),
            received_at: 1_782_986_400,
        }
    }

    #[test]
    fn subjects_prefix_exactly_once() {
        assert_eq!(reply_subject("Hello"), "Re: Hello");
        assert_eq!(reply_subject("Re: Hello"), "Re: Hello");
        assert_eq!(reply_subject("RE: Hello"), "RE: Hello");
        assert_eq!(reply_subject("  re: Hello"), "re: Hello");
        assert_eq!(reply_subject("Fw: Hello"), "Re: Fw: Hello");
        assert_eq!(forward_subject("Hello"), "Fw: Hello");
        assert_eq!(forward_subject("Fwd: Hello"), "Fwd: Hello");
        assert_eq!(forward_subject("FW: Hello"), "FW: Hello");
        assert_eq!(forward_subject("Re: Hello"), "Fw: Re: Hello");
    }

    #[test]
    fn references_chain_extends_the_original() {
        // Original had its own chain → ours appends its message-id.
        let refs = build_references(
            &["root-1@mail.example".into(), "mid-2@mail.example".into()],
            "<orig-123@mail.example>",
        );
        assert_eq!(
            refs,
            [
                "root-1@mail.example",
                "mid-2@mail.example",
                "orig-123@mail.example"
            ]
        );

        // No prior chain → References is just the original's id.
        let refs = build_references(&[], "<orig-123@mail.example>");
        assert_eq!(refs, ["orig-123@mail.example"]);

        // Duplicate protection.
        let refs = build_references(&["orig-123@mail.example".into()], "orig-123@mail.example");
        assert_eq!(refs, ["orig-123@mail.example"]);
    }

    #[test]
    fn parse_references_handles_brackets_and_whitespace() {
        assert_eq!(
            parse_references("<a@x>  <b@y>\r\n\t<c@z>"),
            ["a@x", "b@y", "c@z"]
        );
    }

    #[test]
    fn reply_targets_reply_to_over_from() {
        let mut c = ctx();
        let (to, cc) = reply_recipients(&c, "me@example.com", ComposeMode::Reply);
        assert_eq!(to, vec![addr("Alice", "alice@example.com")]);
        assert!(cc.is_empty());

        c.reply_to = vec![addr("List", "list@example.com")];
        let (to, _) = reply_recipients(&c, "me@example.com", ComposeMode::Reply);
        assert_eq!(to, vec![addr("List", "list@example.com")]);
    }

    #[test]
    fn reply_all_excludes_self_and_drops_nobody_else() {
        let (to, cc) = reply_recipients(&ctx(), "ME@EXAMPLE.COM", ComposeMode::ReplyAll);
        let to_addrs: Vec<_> = to.iter().map(|a| a.address.as_str()).collect();
        let cc_addrs: Vec<_> = cc.iter().map(|a| a.address.as_str()).collect();
        // Sender + other original recipients, self gone (case-insensitive).
        assert_eq!(to_addrs, ["alice@example.com", "bob@example.com"]);
        // Carol kept; the self cc (case-variant) gone.
        assert_eq!(cc_addrs, ["carol@example.com"]);
    }

    #[test]
    fn reply_all_to_own_sent_mail_falls_back_to_original_recipients() {
        let c = ReplyContext {
            from: Some(addr("Me", "me@example.com")),
            to: vec![addr("Bob", "bob@example.com")],
            ..ctx()
        };
        let (to, _) = reply_recipients(&c, "me@example.com", ComposeMode::ReplyAll);
        assert_eq!(to, vec![addr("Bob", "bob@example.com")]);
    }

    #[test]
    fn quoted_html_is_sanitized_and_attributed() {
        let q = quote_html(
            r#"<p>hi</p><script>alert(1)</script><img src=x onerror=alert(1)>"#,
            "Eve <script>",
            "eve@example.com",
            1_782_986_400,
        );
        assert!(q.contains("<blockquote>"));
        assert!(q.contains("wrote:"));
        assert!(!q.to_lowercase().contains("<script"));
        assert!(!q.to_lowercase().contains("onerror"));
        // Attribution content is escaped too.
        assert!(q.contains("Eve &lt;script&gt;"));
    }

    #[test]
    fn quoted_text_prefixes_lines() {
        let q = quote_text("line one\nline two", "Alice", "alice@example.com", 0);
        assert!(q.contains("> line one\n> line two\n"));
    }

    #[test]
    fn mime_carries_threading_headers_and_parts() {
        let draft = Draft {
            to: vec![addr("Bob", "bob@example.com")],
            cc: vec![],
            bcc: vec![],
            subject: "Re: Quarterly numbers".into(),
            body: "<p>Looks right.</p>".into(),
            content_type: "html".into(),
            in_reply_to: Some("orig-123@mail.example".into()),
            references: vec!["root-1@mail.example".into(), "orig-123@mail.example".into()],
            attachments: vec![DraftAttachment {
                name: "notes.txt".into(),
                content_type: "text/plain".into(),
                content_base64: BASE64.encode("hello attachment"),
            }],
        };
        let mime = build_mime(&draft, &addr("Me", "me@example.com")).unwrap();
        let text = String::from_utf8_lossy(&mime);
        assert!(
            text.contains("In-Reply-To: <orig-123@mail.example>"),
            "{text}"
        );
        assert!(
            text.contains("References: <root-1@mail.example> <orig-123@mail.example>"),
            "{text}"
        );
        assert!(
            text.contains("To: \"Bob\" <bob@example.com>")
                || text.contains("To: Bob <bob@example.com>"),
            "{text}"
        );
        assert!(text.contains("Subject: Re: Quarterly numbers"));
        assert!(text.contains("notes.txt"));
        assert!(text.contains("text/html"));
    }

    #[test]
    fn attachment_cap_is_enforced_with_clear_error() {
        let oversized = DraftAttachment {
            name: "big.bin".into(),
            content_type: "application/octet-stream".into(),
            content_base64: BASE64.encode(vec![0u8; MAX_ATTACHMENT_TOTAL_BYTES + 1]),
        };
        let err = validate_attachments(std::slice::from_ref(&oversized)).unwrap_err();
        assert!(matches!(err, ComposeError::AttachmentsTooLarge { .. }));
        assert!(err.to_string().contains("later phase"));

        let ok = DraftAttachment {
            name: "small.bin".into(),
            content_type: "application/octet-stream".into(),
            content_base64: BASE64.encode(vec![0u8; 1024]),
        };
        assert!(validate_attachments(std::slice::from_ref(&ok)).is_ok());
    }

    #[test]
    fn empty_recipients_are_rejected() {
        let draft = Draft {
            to: vec![],
            cc: vec![],
            bcc: vec![],
            subject: "x".into(),
            body: "y".into(),
            content_type: "text".into(),
            in_reply_to: None,
            references: vec![],
            attachments: vec![],
        };
        assert!(matches!(
            build_mime(&draft, &addr("Me", "me@example.com")),
            Err(ComposeError::NoRecipients)
        ));
    }
}
