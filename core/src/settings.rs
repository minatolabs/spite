//! Mailbox settings domain (Phase 8A): out-of-office (automatic replies),
//! master categories, and the read-only working-hours/timezone/date-format
//! block. These types serde-map directly onto the Microsoft Graph
//! `mailboxSettings` and `outlookCategory` shapes so the same struct both
//! deserializes a `GET` and serializes a `PATCH`/`POST` body.

use serde::{Deserialize, Serialize};

use crate::sanitize::sanitize_html;

/// Graph `dateTimeTimeZone`: a wall-clock string plus the zone it's read in
/// (e.g. `{ "dateTime": "2026-07-06T09:00:00.0000000", "timeZone": "UTC" }`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DateTimeTimeZone {
    pub date_time: String,
    pub time_zone: String,
}

/// Graph `automaticRepliesSetting`. `status` is one of `Disabled`,
/// `AlwaysEnabled`, `Scheduled`; `external_audience` is `None`,
/// `ContactsOnly`, or `All` (Graph is case-insensitive on write, and we keep
/// these as plain strings so a `GET` round-trips regardless of casing). The
/// two reply bodies are HTML and MUST be sanitized before leaving the app —
/// see [`AutomaticReplies::sanitized`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomaticReplies {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub external_audience: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduled_start_date_time: Option<DateTimeTimeZone>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduled_end_date_time: Option<DateTimeTimeZone>,
    #[serde(default)]
    pub internal_reply_message: String,
    #[serde(default)]
    pub external_reply_message: String,
}

impl AutomaticReplies {
    /// A copy with both reply bodies passed through the shared `ammonia`
    /// sanitizer. Called on the write path (and safe to call on read) so no
    /// unsanitized HTML is ever stored or PATCHed to Graph.
    pub fn sanitized(&self) -> AutomaticReplies {
        AutomaticReplies {
            internal_reply_message: sanitize_html(&self.internal_reply_message),
            external_reply_message: sanitize_html(&self.external_reply_message),
            ..self.clone()
        }
    }
}

/// Graph `workingHours` (read-only for Spite; the calendar phase is the real
/// consumer). `time_zone` here is a Windows zone name like
/// `"Pacific Standard Time"`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkingHoursInfo {
    #[serde(default)]
    pub days_of_week: Vec<String>,
    #[serde(default)]
    pub start_time: String,
    #[serde(default)]
    pub end_time: String,
    #[serde(default)]
    pub time_zone: Option<NamedTimeZone>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NamedTimeZone {
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocaleInfo {
    #[serde(default)]
    pub locale: String,
    #[serde(default)]
    pub display_name: String,
}

/// The `GET /me/mailboxSettings` payload (the subset Spite reads). Everything
/// is defaulted so a sparse response — or a cached blob from an older shape —
/// still deserializes.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailboxSettings {
    #[serde(default)]
    pub automatic_replies_setting: AutomaticReplies,
    #[serde(default)]
    pub time_zone: String,
    #[serde(default)]
    pub date_format: String,
    #[serde(default)]
    pub time_format: String,
    #[serde(default)]
    pub working_hours: Option<WorkingHoursInfo>,
    #[serde(default)]
    pub language: Option<LocaleInfo>,
}

/// Graph `outlookCategory`. `color` is a preset constant (`preset0`..`preset24`
/// or `None`); `display_name` is immutable in Graph (a "rename" is really a
/// delete + recreate, which we deliberately don't offer).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MasterCategory {
    #[serde(default)]
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub color: String,
}

/// The Graph preset constants Spite offers in its category picker — a curated
/// brass/verdigris family. Reds/cranberries (`preset0/9/15/24`) and everything
/// else are deliberately excluded: oxblood is reserved for the app's own
/// accent, and we never store a color outside Graph's own preset set. The UI
/// renders anything outside this list as a neutral chip.
pub const OFFERED_PRESETS: [&str; 6] = [
    "preset1",  // Orange   → brass
    "preset3",  // Yellow   → old gold
    "preset2",  // Brown    → bronze
    "preset5",  // Teal     → verdigris
    "preset4",  // Green    → patina
    "preset20", // DarkTeal → deep verdigris
];

/// Whether a preset is one Spite is willing to create/recolor a category with.
/// The write commands validate against this so a category never lands on a
/// color outside the curated set.
pub fn is_offered_preset(color: &str) -> bool {
    OFFERED_PRESETS.contains(&color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mailbox_settings_round_trips_graph_shape() {
        let json = serde_json::json!({
            "automaticRepliesSetting": {
                "status": "Scheduled",
                "externalAudience": "All",
                "scheduledStartDateTime": { "dateTime": "2026-07-06T07:00:00.0000000", "timeZone": "UTC" },
                "scheduledEndDateTime": { "dateTime": "2026-07-10T07:00:00.0000000", "timeZone": "UTC" },
                "internalReplyMessage": "<p>Away, internal.</p>",
                "externalReplyMessage": "<p>Away, external.</p>"
            },
            "timeZone": "Pacific Standard Time",
            "dateFormat": "MM/dd/yyyy",
            "timeFormat": "hh:mm tt",
            "workingHours": {
                "daysOfWeek": ["monday", "tuesday"],
                "startTime": "08:00:00.0000000",
                "endTime": "17:00:00.0000000",
                "timeZone": { "name": "Pacific Standard Time" }
            },
            "language": { "locale": "en-US", "displayName": "English (United States)" }
        });
        let s: MailboxSettings = serde_json::from_value(json).unwrap();
        assert_eq!(s.automatic_replies_setting.status, "Scheduled");
        assert_eq!(s.automatic_replies_setting.external_audience, "All");
        assert_eq!(s.time_zone, "Pacific Standard Time");
        assert_eq!(s.working_hours.as_ref().unwrap().days_of_week.len(), 2);
        assert_eq!(s.language.as_ref().unwrap().locale, "en-US");

        // Re-serialize the automaticRepliesSetting for a PATCH: keys stay camelCase.
        let back = serde_json::to_value(&s.automatic_replies_setting).unwrap();
        assert_eq!(back["externalAudience"], "All");
        assert_eq!(back["scheduledStartDateTime"]["timeZone"], "UTC");
    }

    #[test]
    fn sparse_mailbox_settings_deserializes() {
        // A disabled mailbox often omits the schedule and working hours entirely.
        let s: MailboxSettings = serde_json::from_value(serde_json::json!({
            "automaticRepliesSetting": { "status": "Disabled", "externalAudience": "None" },
            "timeZone": "UTC"
        }))
        .unwrap();
        assert_eq!(s.automatic_replies_setting.status, "Disabled");
        assert!(s
            .automatic_replies_setting
            .scheduled_start_date_time
            .is_none());
        assert!(s.working_hours.is_none());
    }

    #[test]
    fn disabled_replies_omit_schedule_when_serialized() {
        let replies = AutomaticReplies {
            status: "Disabled".into(),
            external_audience: "None".into(),
            ..Default::default()
        };
        let v = serde_json::to_value(&replies).unwrap();
        assert!(v.get("scheduledStartDateTime").is_none());
        assert!(v.get("scheduledEndDateTime").is_none());
    }

    #[test]
    fn sanitized_strips_scripts_from_both_bodies() {
        let replies = AutomaticReplies {
            status: "AlwaysEnabled".into(),
            external_audience: "All".into(),
            internal_reply_message: "<p>Hi</p><script>alert(1)</script>".into(),
            external_reply_message: "<p>Out</p><img src=x onerror=alert(2)>".into(),
            ..Default::default()
        };
        let clean = replies.sanitized();
        assert!(!clean.internal_reply_message.contains("<script"));
        assert!(clean.internal_reply_message.contains("<p>Hi</p>"));
        assert!(!clean.external_reply_message.contains("onerror"));
        // Non-body fields are untouched.
        assert_eq!(clean.status, "AlwaysEnabled");
        assert_eq!(clean.external_audience, "All");
    }

    #[test]
    fn master_category_round_trips() {
        let c: MasterCategory = serde_json::from_value(serde_json::json!({
            "id": "abc", "displayName": "Project X", "color": "preset5"
        }))
        .unwrap();
        assert_eq!(c.display_name, "Project X");
        assert_eq!(c.color, "preset5");
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["displayName"], "Project X");
    }

    #[test]
    fn only_curated_presets_are_offered() {
        for p in OFFERED_PRESETS {
            assert!(is_offered_preset(p), "{p} should be offered");
        }
        // Reserved reds/cranberries and `None` are never offered — oxblood stays
        // the app's own accent.
        for p in ["preset0", "preset9", "preset15", "preset24", "None", ""] {
            assert!(!is_offered_preset(p), "{p} must not be offered");
        }
    }
}
