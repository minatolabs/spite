//! Message rules domain (Phase 8B): the full Graph `messageRule` surface.
//!
//! The correctness core is **round-trip safety**. Spite edits the same rules
//! Outlook shows, and a Graph PATCH replaces the `conditions`/`exceptions`/
//! `actions` complex properties *whole* when they're included — so a save must
//! carry those objects complete, including any field this code doesn't know
//! about. Two rules enforce that here:
//!
//! 1. Every struct carries `#[serde(flatten)] extra` so unknown fields ride
//!    through deserialize → serialize verbatim, at every nesting level.
//! 2. Every known field is `Option` + `skip_serializing_if` so an absent
//!    field is *omitted*, never written as an explicit `null` (which would
//!    actively clear it server-side).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Graph `messageRule`. `id`, `has_error`, and `is_read_only` are server-owned
/// (see [`patch_body`], which never writes them back).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_enabled: Option<bool>,
    /// Read-only: the rule can't be modified or deleted via the API.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_read_only: Option<bool>,
    /// Read-only: the rule is in an error condition server-side.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_error: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conditions: Option<RulePredicates>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exceptions: Option<RulePredicates>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actions: Option<RuleActions>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Graph `messageRulePredicates` — the complete condition/exception surface.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RulePredicates {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_addresses: Option<Vec<Recipient>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_to_addresses: Option<Vec<Recipient>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_to_me: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_only_to_me: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_cc_me: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_to_or_cc_me: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_sent_to_me: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipient_contains: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_contains: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_contains: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_contains: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_or_subject_contains: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header_contains: Option<Vec<String>>,
    /// `low` | `normal` | `high`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub importance: Option<String>,
    /// `normal` | `personal` | `private` | `confidential`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sensitivity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_action_flag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_approval_request: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_automatic_forward: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_automatic_reply: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_encrypted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_meeting_request: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_meeting_response: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_non_delivery_report: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_permission_controlled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_read_receipt: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_signed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_voicemail: Option<bool>,
    /// Sizes in kilobytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub within_size_range: Option<SizeRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_attachments: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub categories: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Graph `messageRuleActions` — the complete action surface.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleActions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub move_to_folder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy_to_folder: Option<String>,
    /// Soft delete — moves to Deleted Items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delete: Option<bool>,
    /// Hard delete — skips Deleted Items, unrecoverable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permanent_delete: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forward_to: Option<Vec<Recipient>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forward_as_attachment_to: Option<Vec<Recipient>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_to: Option<Vec<Recipient>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assign_categories: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark_as_read: Option<bool>,
    /// `low` | `normal` | `high`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark_importance: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_processing_rules: Option<bool>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Graph `recipient`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Recipient {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email_address: Option<RuleEmailAddress>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleEmailAddress {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Graph `sizeRange` — kilobytes.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SizeRange {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_size: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum_size: Option<i32>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Serialize a predicates object for a write. `None` **and** empty both map
/// to `Value::Null`: PATCH replaces the complex property whole, so a rule the
/// user emptied must clear deterministically rather than being omitted (which
/// would silently keep the old conditions).
fn predicates_value(p: Option<&RulePredicates>) -> Value {
    match p.map(|p| serde_json::to_value(p).unwrap_or(Value::Null)) {
        Some(Value::Object(m)) if !m.is_empty() => Value::Object(m),
        _ => Value::Null,
    }
}

/// The PATCH body: exactly the six writable properties, with the complex
/// objects carried whole (extras included). Server-owned fields (`id`,
/// `hasError`, `isReadOnly`) and rule-level extras are never written back —
/// echoing them risks 400s and they're not ours to write.
pub fn patch_body(rule: &MessageRule) -> Value {
    let mut body = Map::new();
    if let Some(name) = &rule.display_name {
        body.insert("displayName".into(), Value::String(name.clone()));
    }
    if let Some(seq) = rule.sequence {
        body.insert("sequence".into(), Value::from(seq));
    }
    if let Some(enabled) = rule.is_enabled {
        body.insert("isEnabled".into(), Value::Bool(enabled));
    }
    body.insert(
        "conditions".into(),
        predicates_value(rule.conditions.as_ref()),
    );
    body.insert(
        "exceptions".into(),
        predicates_value(rule.exceptions.as_ref()),
    );
    if let Some(actions) = &rule.actions {
        body.insert(
            "actions".into(),
            serde_json::to_value(actions).unwrap_or(Value::Null),
        );
    }
    Value::Object(body)
}

/// The POST body for creating a rule. Same writable set; empty predicate
/// objects are *omitted* on create (there's nothing to clear yet).
pub fn create_body(rule: &MessageRule) -> Value {
    let mut body = Map::new();
    if let Some(name) = &rule.display_name {
        body.insert("displayName".into(), Value::String(name.clone()));
    }
    if let Some(seq) = rule.sequence {
        body.insert("sequence".into(), Value::from(seq));
    }
    if let Some(enabled) = rule.is_enabled {
        body.insert("isEnabled".into(), Value::Bool(enabled));
    }
    for (key, preds) in [
        ("conditions", rule.conditions.as_ref()),
        ("exceptions", rule.exceptions.as_ref()),
    ] {
        if let Value::Object(m) = predicates_value(preds) {
            body.insert(key.into(), Value::Object(m));
        }
    }
    if let Some(actions) = &rule.actions {
        body.insert(
            "actions".into(),
            serde_json::to_value(actions).unwrap_or(Value::Null),
        );
    }
    Value::Object(body)
}

/// Renumber rules to a contiguous `1..=n` sequence in `new_order`, returning
/// only `(id, new_sequence)` pairs that actually change. Contiguity means no
/// duplicate or gapped slots by construction; ids missing from `new_order`
/// (shouldn't happen — the UI reorders the full list) are left untouched.
pub fn sequence_patches(rules: &[MessageRule], new_order: &[String]) -> Vec<(String, i32)> {
    let current: std::collections::HashMap<&str, Option<i32>> = rules
        .iter()
        .filter_map(|r| r.id.as_deref().map(|id| (id, r.sequence)))
        .collect();
    new_order
        .iter()
        .enumerate()
        .filter_map(|(i, id)| {
            let want = (i + 1) as i32;
            match current.get(id.as_str()) {
                Some(&seq) if seq != Some(want) => Some((id.clone(), want)),
                _ => None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A rule JSON with unknown fields injected at every nesting level.
    fn rule_with_unknowns() -> Value {
        json!({
            "id": "r1",
            "displayName": "From partner",
            "sequence": 3,
            "isEnabled": true,
            "futureRuleField": {"nested": true},
            "conditions": {
                "senderContains": ["adele"],
                "futurePredicate": ["x"],
                "fromAddresses": [{
                    "emailAddress": { "name": "A", "address": "a@x.com", "futureAddrField": 1 },
                    "futureRecipientField": "keep"
                }],
                "withinSizeRange": { "minimumSize": 10, "maximumSize": 500, "futureSizeField": 2 }
            },
            "actions": {
                "moveToFolder": "folder-id",
                "futureAction": { "sendTo": "hidden@evil.com" }
            }
        })
    }

    #[test]
    fn round_trip_preserves_unknown_fields_at_every_level() {
        let rule: MessageRule = serde_json::from_value(rule_with_unknowns()).unwrap();
        let back = serde_json::to_value(&rule).unwrap();
        assert_eq!(back["futureRuleField"]["nested"], true);
        assert_eq!(back["conditions"]["futurePredicate"][0], "x");
        assert_eq!(
            back["conditions"]["fromAddresses"][0]["futureRecipientField"],
            "keep"
        );
        assert_eq!(
            back["conditions"]["fromAddresses"][0]["emailAddress"]["futureAddrField"],
            1
        );
        assert_eq!(back["conditions"]["withinSizeRange"]["futureSizeField"], 2);
        // The smuggling-shaped case: an unrecognized action must survive.
        assert_eq!(back["actions"]["futureAction"]["sendTo"], "hidden@evil.com");
        // Known fields survive alongside.
        assert_eq!(back["conditions"]["senderContains"][0], "adele");
        assert_eq!(back["actions"]["moveToFolder"], "folder-id");
    }

    #[test]
    fn absent_fields_are_omitted_not_null() {
        let rule: MessageRule = serde_json::from_value(json!({
            "id": "r1",
            "displayName": "Sparse",
            "conditions": { "hasAttachments": true },
            "actions": { "markAsRead": true }
        }))
        .unwrap();
        let back = serde_json::to_value(&rule).unwrap();
        // No explicit nulls anywhere — a null would clear server state.
        let conditions = back["conditions"].as_object().unwrap();
        assert_eq!(conditions.len(), 1, "only hasAttachments: {conditions:?}");
        let actions = back["actions"].as_object().unwrap();
        assert_eq!(actions.len(), 1, "only markAsRead: {actions:?}");
        assert!(back.get("exceptions").is_none());
        assert!(back.get("sequence").is_none());
    }

    #[test]
    fn maximal_rule_round_trips_equal() {
        // Every predicate and every action set — the full surface.
        let recipient = json!([{ "emailAddress": { "name": "N", "address": "n@x.com" } }]);
        let maximal = json!({
            "id": "max", "displayName": "Everything", "sequence": 1,
            "isEnabled": true, "isReadOnly": false, "hasError": false,
            "conditions": {
                "fromAddresses": recipient, "sentToAddresses": recipient,
                "sentToMe": true, "sentOnlyToMe": false, "sentCcMe": true,
                "sentToOrCcMe": true, "notSentToMe": false,
                "recipientContains": ["r"], "senderContains": ["s"],
                "subjectContains": ["subj"], "bodyContains": ["b"],
                "bodyOrSubjectContains": ["bs"], "headerContains": ["h"],
                "importance": "high", "sensitivity": "private",
                "messageActionFlag": "followUp", "isApprovalRequest": true,
                "isAutomaticForward": true, "isAutomaticReply": true,
                "isEncrypted": true, "isMeetingRequest": true,
                "isMeetingResponse": true, "isNonDeliveryReport": true,
                "isPermissionControlled": true, "isReadReceipt": true,
                "isSigned": true, "isVoicemail": true,
                "withinSizeRange": { "minimumSize": 1, "maximumSize": 1024 },
                "hasAttachments": true, "categories": ["Brass"]
            },
            "exceptions": { "subjectContains": ["skip"] },
            "actions": {
                "moveToFolder": "f1", "copyToFolder": "f2",
                "delete": false, "permanentDelete": false,
                "forwardTo": recipient, "forwardAsAttachmentTo": recipient,
                "redirectTo": recipient, "assignCategories": ["Brass"],
                "markAsRead": true, "markImportance": "low",
                "stopProcessingRules": true
            }
        });
        let rule: MessageRule = serde_json::from_value(maximal.clone()).unwrap();
        // Nothing landed in an `extra` bucket — the typed surface is complete.
        assert!(rule.extra.is_empty());
        let c = rule.conditions.as_ref().unwrap();
        assert!(c.extra.is_empty(), "unmapped predicate keys: {:?}", c.extra);
        let a = rule.actions.as_ref().unwrap();
        assert!(a.extra.is_empty(), "unmapped action keys: {:?}", a.extra);
        // And the value round-trips exactly.
        let again: MessageRule =
            serde_json::from_value(serde_json::to_value(&rule).unwrap()).unwrap();
        assert_eq!(rule, again);
    }

    #[test]
    fn patch_body_carries_whole_objects_and_excludes_server_fields() {
        let rule: MessageRule = serde_json::from_value(rule_with_unknowns()).unwrap();
        let body = patch_body(&rule);
        // Writable props present…
        assert_eq!(body["displayName"], "From partner");
        assert_eq!(body["sequence"], 3);
        assert_eq!(body["isEnabled"], true);
        // …complex objects whole, extras included (round-trip safety)…
        assert_eq!(body["conditions"]["futurePredicate"][0], "x");
        assert_eq!(body["actions"]["futureAction"]["sendTo"], "hidden@evil.com");
        // …server-owned fields and rule-level extras never written back.
        assert!(body.get("id").is_none());
        assert!(body.get("hasError").is_none());
        assert!(body.get("isReadOnly").is_none());
        assert!(body.get("futureRuleField").is_none());
        // No exceptions on this rule → explicit null clears deterministically.
        assert!(body["exceptions"].is_null());
    }

    #[test]
    fn emptied_predicates_patch_as_null_but_are_omitted_on_create() {
        let rule = MessageRule {
            display_name: Some("Bare".into()),
            is_enabled: Some(true),
            conditions: Some(RulePredicates::default()), // user removed them all
            actions: Some(RuleActions {
                mark_as_read: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        };
        let patch = patch_body(&rule);
        assert!(patch["conditions"].is_null(), "PATCH must clear explicitly");
        let create = create_body(&rule);
        assert!(create.get("conditions").is_none(), "POST omits empties");
        assert_eq!(create["actions"]["markAsRead"], true);
    }

    #[test]
    fn sequence_patches_renumber_contiguously_and_minimally() {
        let rule = |id: &str, seq: i32| MessageRule {
            id: Some(id.into()),
            sequence: Some(seq),
            ..Default::default()
        };
        // Gapped + duplicated input order: a=2, b=2, c=7.
        let rules = vec![rule("a", 2), rule("b", 2), rule("c", 7)];
        // User drags c to the top: c, a, b → 1, 2, 3.
        let order = vec!["c".to_string(), "a".to_string(), "b".to_string()];
        let patches = sequence_patches(&rules, &order);
        // c: 7→1, b: 2→3. a already sits at 2 — untouched (minimal writes).
        assert_eq!(patches, vec![("c".to_string(), 1), ("b".to_string(), 3)]);
        // Renumbering the same order twice is a no-op.
        let renumbered = vec![rule("c", 1), rule("a", 2), rule("b", 3)];
        assert!(sequence_patches(&renumbered, &order).is_empty());
    }
}
