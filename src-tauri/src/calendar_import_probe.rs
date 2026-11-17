//! Explicitly gated, read-only preflight for the user's Scadix invitation.
//! Does not create events or send RSVP. Never print authenticated source data.
use crate::{
    config::TestConfig,
    sync::{self, Remote},
};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::PathBuf};
fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.cache")
}
fn config() -> TestConfig {
    if std::env::var_os("JMAP_TEST_REGISTRY").is_some() {
        return crate::upload_roundtrip::test_config()
            .expect("Explicit native profile snapshot unavailable");
    }
    let profile = std::env::var("JMAP_TEST_PROFILE").expect("Explicit profile selection required");
    let account = std::env::var("JMAP_TEST_ACCOUNT").expect("Explicit account selection required");
    TestConfig::existing_for_test(&profile, &account)
        .expect("Existing native credential unavailable")
}
async fn get(remote: &impl Remote, method: &str, args: Value) -> Value {
    remote
        .call(method, args)
        .await
        .unwrap_or_else(|_| panic!("Read-only calendar preflight request failed"))
}
fn attachments(value: &Value, out: &mut BTreeMap<String, crate::blobs::BlobRef>) {
    match value {
        Value::Object(map) => {
            let kind = map.get("type").and_then(Value::as_str).unwrap_or("");
            let name = map.get("name").and_then(Value::as_str).unwrap_or("");
            if (kind.eq_ignore_ascii_case("text/calendar")
                || name.to_ascii_lowercase().ends_with(".ics"))
                && map.get("blobId").is_some_and(Value::is_string)
            {
                let id = map["blobId"].as_str().unwrap().to_owned();
                out.insert(
                    id.clone(),
                    crate::blobs::BlobRef {
                        id,
                        media_type: kind.into(),
                        size: map.get("size").and_then(Value::as_u64),
                        attachment: true,
                    },
                );
            }
            for (key, child) in map {
                if key != "bodyValues" {
                    attachments(child, out);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                attachments(item, out);
            }
        }
        _ => {}
    }
}
#[test]
#[ignore = "Read-only preflight for the specifically requested invitation; requires JMAP_CALENDAR_PREFLIGHT=scadix"]
fn live_scadix_calendar_preflight() {
    assert_eq!(
        std::env::var("JMAP_CALENDAR_PREFLIGHT").as_deref(),
        Ok("scadix")
    );
    tauri::async_runtime::block_on(async {
        let mut config = config();
        config.read_only = true;
        let session: Value = serde_json::from_slice(
            &crate::session::session_bytes(&config)
                .await
                .expect("Read-only session discovery failed"),
        )
        .expect("Session invalid");
        let mail = session["primaryAccounts"]["urn:ietf:params:jmap:mail"]
            .as_str()
            .expect("Personal mail account unavailable");
        let calendar = session["primaryAccounts"][sync::CALENDARS]
            .as_str()
            .expect("Personal calendar account unavailable");
        let source = sync::Connection::discover(&config, mail)
            .await
            .unwrap_or_else(|_| panic!("Mail discovery failed"));
        let query=get(&source,"Email/query",json!({"accountId":mail,"filter":{"from":"scadix","hasAttachment":true},"sort":[{"property":"receivedAt","isAscending":false}],"limit":20,"calculateTotal":true})).await;
        let ids = query["ids"].as_array().expect("Invalid query");
        let emails=get(&source,"Email/get",json!({"accountId":mail,"ids":ids,"properties":["id","from","attachments","bodyStructure","receivedAt"]})).await;
        let mut blobs = BTreeMap::new();
        attachments(&emails["list"], &mut blobs);
        assert!(
            blobs.len() <= 20,
            "Too many candidate invitations; narrow source before proceeding"
        );
        let mut candidates = BTreeMap::new();
        for reference in blobs.values() {
            if reference.size.is_some_and(|n| n > 2 * 1024 * 1024) {
                continue;
            }
            let bytes = source
                .download_blob(mail, reference)
                .await
                .unwrap_or_else(|_| panic!("Read-only calendar blob download failed"));
            if let Ok(events) = crate::calendar_parse::entries(&bytes, None) {
                for event in events {
                    if let Some(uid) = event["uid"].as_str().map(str::to_owned) {
                        candidates.entry(uid).or_insert(event);
                    }
                }
            }
        }
        let target = sync::Connection::discover_for(&config, calendar, sync::CALENDARS)
            .await
            .unwrap_or_else(|_| panic!("Calendar discovery failed"));
        let calendars = get(
            &target,
            "Calendar/get",
            json!({"accountId":calendar,"ids":null}),
        )
        .await;
        let defaults: Vec<&Value> = calendars["list"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["isDefault"] == true && c["myRights"]["mayWriteAll"] == true)
            .collect();
        let mut report = json!({"status":"ambiguous","candidateInvitations":candidates.len(),"defaultCalendars":defaults.len()});
        if candidates.len() == 1 && defaults.len() == 1 {
            let (uid, event) = candidates.into_iter().next().unwrap();
            let duplicate = get(
                &target,
                "CalendarEvent/query",
                json!({"accountId":calendar,"filter":{"uid":uid},"limit":2,"calculateTotal":true}),
            )
            .await;
            let exists = duplicate["ids"]
                .as_array()
                .is_some_and(|ids| !ids.is_empty());
            let mut in_default = false;
            let mut schedule_matches = false;
            if exists {
                let stored=get(&target,"CalendarEvent/get",json!({"accountId":calendar,"ids":duplicate["ids"],"properties":["id","uid","calendarIds","start","timeZone","duration","showWithoutTime"]})).await;
                for stored in stored["list"]
                    .as_array()
                    .expect("Calendar readback invalid")
                {
                    if stored["uid"] == uid
                        && stored["calendarIds"][defaults[0]["id"].as_str().unwrap()] == true
                    {
                        in_default = true;
                        schedule_matches = stored["start"] == event["start"]
                            && stored["timeZone"] == event["timeZone"]
                            && stored["duration"] == event["duration"];
                    }
                }
            }
            report = json!({"status":if in_default && schedule_matches {"already-in-default"}else if exists{"existing-needs-review"}else{"ready"},"candidateInvitations":1,"defaultCalendars":1,
                "inDefaultCalendar":in_default,"scheduleMatches":schedule_matches});
        }
        std::fs::write(
            root().join("scadix-calendar-preflight.json"),
            serde_json::to_vec_pretty(&report).unwrap(),
        )
        .unwrap();
        eprintln!(
            "Calendar preflight: status={}, candidates={}, writable defaults={}",
            report["status"].as_str().unwrap(),
            report["candidateInvitations"],
            report["defaultCalendars"]
        );
    });
}
