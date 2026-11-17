//! Read-only CalendarEvent/parse backed by account-authorized redb blobs.
//! Conversion follows Stalwart calendar_event/parse.rs, using calcard directly.
use crate::{config::TestConfig, storage::Store};
use base64::{engine::general_purpose::STANDARD, Engine};
use calcard::icalendar::ICalendar;
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use tauri::State;

const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_BLOBS: usize = 16;
const MAX_EVENTS: usize = 256;

fn load(
    store: &Store,
    config: &TestConfig,
    account: &str,
    ids: &[String],
) -> Result<Vec<(String, Option<Vec<u8>>)>, String> {
    if ids.len() > MAX_BLOBS {
        return Err("requestTooLarge".into());
    }
    if ids.iter().any(|id| id.is_empty() || id.len() > 4096) {
        return Err("invalidArguments".into());
    }
    crate::desktop::require_capability(store, config, account, "urn:ietf:params:jmap:mail")
        .or_else(|_| crate::desktop::require_capability(store, config, account, crate::sync::FILES))
        .map_err(|_| "accountNotFound".to_string())?;
    let mut seen = HashSet::new();
    let mut blobs = Vec::new();
    for id in ids {
        if !seen.insert(id) {
            continue;
        }
        // Same reference, membership and per-account checks as the blob viewer.
        let bytes = match crate::read_local_blob(store, config, account, id) {
            Ok(Some(blob)) => Some(STANDARD.decode(blob.base64).map_err(|_| "serverFail")?),
            Ok(None) | Err(_) => None,
        };
        if bytes.as_ref().is_some_and(|b| b.len() > MAX_BYTES) {
            return Err("requestTooLarge".into());
        }
        blobs.push((id.clone(), bytes));
    }
    Ok(blobs)
}
pub(crate) fn entries(bytes: &[u8], properties: Option<&[String]>) -> Result<Vec<Value>, ()> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| ())?
        .trim_start_matches('\u{feff}');
    let ical = ICalendar::parse(text).map_err(|_| ())?;
    let group = ical.into_jscalendar::<String, String>();
    let value = serde_json::to_value(group).map_err(|_| ())?;
    let mut entries = value["entries"]
        .as_array()
        .filter(|v| !v.is_empty() && v.len() <= MAX_EVENTS)
        .cloned()
        .ok_or(())?;
    for entry in &mut entries {
        let object = entry.as_object_mut().ok_or(())?;
        if let Some(properties) = properties {
            object.retain(|key, _| properties.contains(key));
        }
        if properties.is_none_or(|p| p.iter().any(|k| k == "version")) {
            object.insert(
                "version".into(),
                value.get("version").cloned().unwrap_or(json!("1.0")),
            );
        }
        for key in ["id", "baseEventId", "calendarIds", "isDraft", "isOrigin"] {
            if properties.is_none_or(|p| p.iter().any(|k| k == key)) {
                object.insert(key.into(), Value::Null);
            }
        }
    }
    Ok(entries)
}
fn answer(
    account: &str,
    blobs: Vec<(String, Option<Vec<u8>>)>,
    properties: Option<Vec<String>>,
) -> Value {
    let mut parsed = Map::new();
    let mut not_found = Vec::new();
    let mut not_parsable = Vec::new();
    for (id, bytes) in blobs {
        let Some(bytes) = bytes else {
            not_found.push(id);
            continue;
        };
        match entries(&bytes, properties.as_deref()) {
            Ok(events) => {
                parsed.insert(id, Value::Array(events));
            }
            Err(()) => not_parsable.push(id),
        }
    }
    json!({"accountId":account,"parsed":parsed,"notFound":not_found,"notParsable":not_parsable})
}
#[tauri::command]
pub async fn local_calendar_parse(
    store: State<'_, Store>,
    account_id: String,
    blob_ids: Vec<String>,
    properties: Option<Vec<String>>,
) -> Result<Value, String> {
    if properties
        .as_ref()
        .is_some_and(|p| p.len() > 256 || p.iter().any(|k| k.is_empty() || k.len() > 256))
    {
        return Err("invalidArguments".into());
    }
    let config = TestConfig::load()?;
    let blobs = load(&store, &config, &account_id, &blob_ids)?;
    tauri::async_runtime::spawn_blocking(move || answer(&account_id, blobs, properties))
        .await
        .map_err(|_| "serverFail".into())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{replica::Collection, sync};
    const ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Herakles Test//EN\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\nUID:synthetic-invite@example.test\r\nDTSTAMP:20261116T100000Z\r\nDTSTART;TZID=Europe/Berlin:20261120T140000\r\nDTEND;TZID=Europe/Berlin:20261120T150000\r\nSUMMARY:Synthetic calendar invitation\r\nLOCATION:Test room\r\nORGANIZER;CN=Test Organizer:mailto:organizer@example.test\r\nATTENDEE;CN=Test Reader;RSVP=TRUE:mailto:reader@example.test\r\nRRULE:FREQ=WEEKLY;COUNT=3\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    #[test]
    fn stalwart_calcard_parses_timezone_recurrence_participants_and_property_selection() {
        let events = entries(ICS.as_bytes(), None).unwrap();
        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert_eq!(event["@type"], "Event");
        assert_eq!(event["uid"], "synthetic-invite@example.test");
        assert_eq!(event["title"], "Synthetic calendar invitation");
        assert_eq!(event["start"], "2026-11-20T14:00:00");
        assert_eq!(event["timeZone"], "Europe/Berlin");
        assert_eq!(event["duration"], "PT1H");
        assert_eq!(event["recurrenceRule"]["frequency"], "weekly");
        assert!(event["participants"]
            .as_object()
            .is_some_and(|p| p.len() >= 2));
        assert!(event["id"].is_null());
        assert!(event["calendarIds"].is_null());
        let selected = entries(ICS.as_bytes(), Some(&["title".into()])).unwrap();
        assert_eq!(
            selected,
            vec![json!({"title":"Synthetic calendar invitation"})]
        );
        let all_day = ICS
            .replace(
                "DTSTART;TZID=Europe/Berlin:20261120T140000",
                "DTSTART;VALUE=DATE:20261120",
            )
            .replace(
                "DTEND;TZID=Europe/Berlin:20261120T150000",
                "DTEND;VALUE=DATE:20261121",
            );
        assert_eq!(
            entries(all_day.as_bytes(), None).unwrap()[0]["showWithoutTime"],
            true
        );
    }
    #[test]
    fn parse_returns_not_parsable_and_not_found_without_panics_or_contents_in_errors() {
        let response = answer(
            "a",
            vec![
                ("valid".into(), Some(ICS.as_bytes().to_vec())),
                ("bad".into(), Some(b"not a calendar".to_vec())),
                ("invalid-utf8".into(), Some(vec![255])),
                ("missing".into(), None),
            ],
            None,
        );
        assert!(response["parsed"]["valid"].is_array());
        assert_eq!(response["notParsable"], json!(["bad", "invalid-utf8"]));
        assert_eq!(response["notFound"], json!(["missing"]));
    }
    #[test]
    fn cached_invitation_parses_offline_after_restart_and_requires_current_mail_reference() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("invitation.redb");
        let config = TestConfig {
            url: url::Url::parse("https://mail.example.test/jmap").unwrap(),
            username: "test".into(),
            password: String::new(),
            read_only: true,
        };
        let scope = sync::scope(&config, "a");
        {
            let store = Store::open(&path).unwrap();
            // Source mail account deliberately has no calendars capability.
            store.put(&format!("{}:desktop-session",config.cache_key()),&json!({"accounts":{"a":{"accountCapabilities":{"urn:ietf:params:jmap:mail":{}}},"b":{"accountCapabilities":{"urn:ietf:params:jmap:mail":{}}}}}).to_string()).unwrap();
            store
                .apply_remote(
                    &scope,
                    Collection::Email,
                    None,
                    "s1",
                    &[json!({"id":"e","attachments":[{"blobId":"invite","type":"text/calendar"}]})],
                    &[],
                    true,
                )
                .unwrap();
            store.cache_blob(&scope, "invite", ICS.as_bytes()).unwrap();
        }
        let store = Store::open(&path).unwrap();
        let blobs = load(&store, &config, "a", &["invite".into(), "invite".into()]).unwrap();
        assert_eq!(blobs.len(), 1);
        assert_eq!(
            answer("a", blobs, None)["parsed"]["invite"][0]["title"],
            "Synthetic calendar invitation"
        );
        assert_eq!(
            answer(
                "b",
                load(&store, &config, "b", &["invite".into()]).unwrap(),
                None
            )["notFound"],
            json!(["invite"])
        );
        assert!(load(&store, &config, "revoked", &["invite".into()]).is_err());
        assert!(load(&store, &config, "a", &vec!["invite".into(); MAX_BLOBS + 1]).is_err());
        store
            .apply_remote(
                &scope,
                Collection::Email,
                Some("s1"),
                "s2",
                &[],
                &["e".into()],
                false,
            )
            .unwrap();
        assert_eq!(
            answer(
                "a",
                load(&store, &config, "a", &["invite".into()]).unwrap(),
                None
            )["notFound"],
            json!(["invite"])
        );
        assert_eq!(
            store.metadata(&scope).unwrap().email_state.as_deref(),
            Some("s2")
        );
    }
}
