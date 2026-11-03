use crate::config::TestConfig;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Session {
    capabilities: BTreeMap<String, serde_json::Value>,
    accounts: BTreeMap<String, ServerAccount>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerAccount {
    name: String,
    is_personal: bool,
    is_read_only: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    id: String,
    name: String,
    is_personal: bool,
    is_read_only: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    accounts: Vec<Account>,
    capabilities: Vec<String>,
    cached_at: u64,
}

pub async fn probe(config: &TestConfig) -> Result<SessionSummary, String> {
    summarize(&session_bytes(config).await?)
}

fn redirect_target(
    original: &url::Url,
    current: &url::Url,
    location: &str,
) -> Result<url::Url, String> {
    let target = current
        .join(location)
        .map_err(|_| "Ungültige JMAP-Umleitung.")?;
    if target.origin() != original.origin()
        || !target.username().is_empty()
        || target.password().is_some()
        || target.query().is_some()
        || target.fragment().is_some()
    {
        return Err(
            "JMAP-Umleitung auf eine fremde oder unzulässige Adresse wurde gesperrt.".into(),
        );
    }
    Ok(target)
}

pub(crate) async fn session_bytes(config: &TestConfig) -> Result<Vec<u8>, String> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|_| "HTTP-Client konnte nicht gestartet werden.")?;
    let mut url = config.url.clone();
    let mut redirects = 0;
    let mut response = loop {
        let response = client
            .get(url.clone())
            .basic_auth(&config.username, Some(&config.password))
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|_| {
                "JMAP-Server nicht erreichbar. Adresse, Netzwerk und TLS-Zertifikat prüfen."
            })?;
        if !response.status().is_redirection() {
            break response;
        }
        if redirects >= 3 {
            return Err("Zu viele JMAP-Umleitungen.".into());
        }
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or("JMAP-Umleitung ohne gültiges Ziel.")?;
        url = redirect_target(&config.url, &url, location)?;
        redirects += 1;
    };
    match response.status().as_u16() {
        200 => (),
        401 | 403 => return Err("Anmeldung abgelehnt. Testzugang prüfen.".into()),
        300..=399 => {
            return Err(
                "Server leitet um. Bitte die endgültige HTTPS-Sitzungsadresse konfigurieren."
                    .into(),
            )
        }
        code => return Err(format!("JMAP-Server antwortet mit HTTP {code}.")),
    }
    // Never include server bodies or reqwest errors in frontend errors: they may contain secrets.
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Serverantwort unvollständig.")?
    {
        if bytes.len() + chunk.len() > 2 * 1024 * 1024 {
            return Err("JMAP-Sitzungsantwort ist zu groß.".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn summarize(bytes: &[u8]) -> Result<SessionSummary, String> {
    let session: Session = serde_json::from_slice(bytes)
        .map_err(|_| "Keine gültige JMAP-Sitzungsantwort erhalten.")?;
    if !session
        .capabilities
        .contains_key("urn:ietf:params:jmap:core")
    {
        return Err("Server meldet keine JMAP-Core-Unterstützung.".into());
    }
    Ok(SessionSummary {
        accounts: session
            .accounts
            .into_iter()
            .map(|(id, account)| Account {
                id,
                name: account.name,
                is_personal: account.is_personal,
                is_read_only: account.is_read_only,
            })
            .collect(),
        capabilities: session.capabilities.into_keys().collect(),
        cached_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "Systemzeit ungültig.")?
            .as_secs(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_redirects_never_forward_credentials_to_another_origin() {
        let base = url::Url::parse("https://mail.example.com/.well-known/jmap").unwrap();
        assert!(redirect_target(&base, &base, "/jmap/session").is_ok());
        for target in [
            "https://other.example.com/jmap",
            "http://mail.example.com/jmap",
            "https://mail.example.com:8443/jmap",
            "https://user@mail.example.com/jmap",
            "/jmap?token=secret",
            "/jmap#fragment",
        ] {
            assert!(redirect_target(&base, &base, target).is_err());
        }
    }

    #[test]
    #[ignore = "Requires .env.local; downloads at most ten messages, never writes to server"]
    fn live_stalwart_read_only_smoke() {
        use crate::{
            replica::Collection,
            storage::Store,
            sync::{Connection, Remote},
        };
        tauri::async_runtime::block_on(async {
            let mut config = TestConfig::load().expect("Lokale Testkonfiguration ungültig.");
            // Force the native transport policy independently of the user's environment file.
            config.read_only = true;
            let summary = probe(&config)
                .await
                .expect("JMAP-Sitzungsprüfung fehlgeschlagen.");
            let account = summary
                .accounts
                .iter()
                .find(|a| a.is_personal)
                .or_else(|| summary.accounts.first())
                .expect("Keine JMAP-Konten vorhanden.");
            let connection = Connection::discover(&config, &account.id)
                .await
                .unwrap_or_else(|error| panic!("{}", error.message()));
            // This must fail locally, before any HTTP request.
            assert!(connection
                .call("Email/set", serde_json::json!({}))
                .await
                .is_err());
            let mailboxes = connection
                .call(
                    "Mailbox/get",
                    serde_json::json!({"accountId":account.id,"ids":null}),
                )
                .await
                .unwrap_or_else(|error| panic!("{}", error.message()));
            let query = connection
                .call(
                    "Email/query",
                    serde_json::json!({"accountId":account.id,"limit":10}),
                )
                .await
                .unwrap_or_else(|error| panic!("{}", error.message()));
            let ids = query["ids"]
                .as_array()
                .expect("Ungültige Nachrichtenliste.");
            assert!(ids.len() <= 10);
            let emails = connection.call("Email/get", serde_json::json!({"accountId":account.id,"ids":ids,
                "properties":["id","subject","keywords","receivedAt","preview","textBody","bodyValues"],
                "fetchTextBodyValues":true,"maxBodyValueBytes":16384}))
                .await.unwrap_or_else(|error| panic!("{}", error.message()));
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("smoke.redb");
            let expected_count;
            {
                let store = Store::open(&path).unwrap();
                let rows = emails["list"]
                    .as_array()
                    .expect("Ungültige Nachrichtenantwort.");
                expected_count = rows.len();
                store
                    .apply_remote(
                        "smoke",
                        Collection::Email,
                        None,
                        emails["state"].as_str().unwrap(),
                        rows,
                        &[],
                        true,
                    )
                    .unwrap();
                store
                    .apply_remote(
                        "smoke",
                        Collection::Mailbox,
                        None,
                        mailboxes["state"].as_str().unwrap(),
                        mailboxes["list"].as_array().unwrap(),
                        &[],
                        true,
                    )
                    .unwrap();
            }
            // Reopen redb and read without any further network access.
            let store = Store::open(&path).unwrap();
            let local = store.local_mail("smoke").unwrap();
            assert!(local.initialized);
            assert!(local.emails.len() == expected_count);
            assert!(local.pending.is_empty());
        });
    }

    #[test]
    fn summary_excludes_credentials_and_unknown_server_fields() {
        let summary = summarize(br#"{"capabilities":{"urn:ietf:params:jmap:core":{}},"accounts":{"a":{"name":"Test","isPersonal":true,"isReadOnly":false}},"accessToken":"DO_NOT_CACHE"}"#).unwrap();
        let json = serde_json::to_string(&summary).unwrap();
        assert!(!json.contains("DO_NOT_CACHE"));
        assert_eq!(summary.accounts[0].id, "a");
    }

    #[test]
    fn rejects_invalid_or_non_jmap_response() {
        assert!(summarize(b"<html>login</html>").is_err());
        assert!(summarize(br#"{"capabilities":{},"accounts":{}}"#).is_err());
    }
}
