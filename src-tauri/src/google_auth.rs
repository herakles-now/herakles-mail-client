//! Native Google OAuth and read-only People API worker. No tokens cross IPC.
use crate::{
    config::TestConfig,
    google_contacts::{self, Source},
    storage::Store,
    NetworkMode, SyncGate,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    net::TcpListener,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::State;

const SCOPE: &str = "openid email https://www.googleapis.com/auth/contacts.readonly";
#[derive(Default)]
pub struct GoogleGate(pub tokio::sync::Mutex<()>);
#[derive(Serialize, Deserialize)]
struct Credential {
    client_id: String,
    client_secret: String,
    refresh_token: String,
    subject: String,
    email: String,
}
fn entry(config: &TestConfig) -> Result<keyring::Entry, String> {
    keyring::Entry::new(
        "org.jmapdesktop.client.google",
        &google_contacts::digest(&config.cache_key()),
    )
    .map_err(|_| "Google-Zugangsspeicher nicht verfügbar.".into())
}
fn load(config: &TestConfig) -> Result<Option<Credential>, String> {
    match entry(config)?.get_password() {
        Ok(s) => serde_json::from_str(&s)
            .map(Some)
            .map_err(|_| "Gespeicherter Google-Zugang ungültig.".into()),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(_) => Err("Google-Zugangsspeicher nicht verfügbar.".into()),
    }
}
pub(crate) fn connected(config: &TestConfig) -> Result<bool, String> {
    Ok(load(config)?.is_some())
}
fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(45))
        .build()
        .map_err(|_| "Google-Verbindung nicht verfügbar.".into())
}
async fn json_response(mut response: reqwest::Response) -> Result<Value, String> {
    if !response.status().is_success() {
        return Err(match response.status().as_u16() {
            400 => "Google hat die Anmeldung oder deren Erneuerung abgelehnt. OAuth-Client prüfen und Google neu verbinden.",
            401 | 403 => "Google-Zugriff verweigert. Anmeldung und People-API-Freigabe prüfen.",
            429 => "Google-Limit erreicht. Bitte später erneut abrufen.",
            _ => "Google-Anfrage fehlgeschlagen. Bitte später erneut versuchen.",
        }
        .into());
    }
    let mut data = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Google-Verbindung unterbrochen.")?
    {
        if data.len() + chunk.len() > 8 * 1024 * 1024 {
            return Err("Google-Antwort überschreitet das Limit.".into());
        }
        data.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&data).map_err(|_| "Ungültige Google-Antwort.".into())
}
fn random() -> Result<String, String> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|_| "Sicherer Zufall nicht verfügbar.")?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
fn validate_client(id: &str, secret: &str) -> Result<(), String> {
    if id.len() > 512
        || !id.ends_with(".apps.googleusercontent.com")
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-._".contains(&c))
        || secret.len() > 1024
        || secret.chars().any(char::is_control)
    {
        return Err("Bitte gültige OAuth-Daten einer Google-Desktop-App eingeben.".into());
    }
    Ok(())
}
fn app_client_values(id: Option<&str>, secret: Option<&str>) -> Result<(String, String), String> {
    let id = id
        .filter(|v| !v.trim().is_empty())
        .ok_or("Die Google-Anmeldung ist in dieser App-Version noch nicht eingerichtet.")?;
    let secret = secret.unwrap_or("");
    validate_client(id, secret)?;
    Ok((id.into(), secret.into()))
}
fn app_client() -> Result<(String, String), String> {
    app_client_values(
        option_env!("HERAKLES_GOOGLE_CLIENT_ID"),
        option_env!("HERAKLES_GOOGLE_CLIENT_SECRET"),
    )
}
pub(crate) fn oauth_configured() -> bool {
    app_client().is_ok()
}

fn authorization_url(client_id: &str, redirect: &str, state: &str, verifier: &str) -> String {
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut url = url::Url::parse("https://accounts.google.com/o/oauth2/v2/auth").unwrap();
    url.query_pairs_mut().extend_pairs([
        ("client_id", client_id),
        ("redirect_uri", redirect),
        ("response_type", "code"),
        ("scope", SCOPE),
        ("state", state),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
        ("access_type", "offline"),
        ("prompt", "consent select_account"),
    ]);
    url.into()
}
fn callback_code(target: &str, host: &str, state: &str) -> Result<String, String> {
    let url = url::Url::parse(&format!("http://{host}{target}"))
        .map_err(|_| "Ungültige Anmeldeantwort.")?;
    if url.path() != "/callback" {
        return Err("Ungültiger Anmeldepfad.".into());
    }
    let params: Vec<_> = url.query_pairs().collect();
    let get = |key: &str| -> Result<String, String> {
        let matches: Vec<_> = params.iter().filter(|(k, _)| k == key).collect();
        if matches.len() != 1 {
            return Err("Ungültige Anmeldeantwort.".into());
        }
        Ok(matches[0].1.to_string())
    };
    if get("state")? != state {
        return Err("Anmeldung konnte nicht sicher zugeordnet werden.".into());
    }
    if params.iter().any(|(k, _)| k == "error") {
        return Err("Google-Anmeldung wurde abgebrochen oder verweigert.".into());
    }
    get("code").and_then(|s| {
        if s.is_empty() || s.len() > 4096 {
            Err("Ungültiger Anmeldecode.".into())
        } else {
            Ok(s)
        }
    })
}
fn await_callback(listener: TcpListener, state: String) -> Result<String, String> {
    listener
        .set_nonblocking(true)
        .map_err(|_| "Anmeldeempfang nicht verfügbar.")?;
    let host = listener
        .local_addr()
        .map_err(|_| "Anmeldeempfang nicht verfügbar.")?
        .to_string();
    let deadline = Instant::now() + Duration::from_secs(300);
    while Instant::now() < deadline {
        match listener.accept() {
            Ok((mut stream, peer)) => {
                if !peer.ip().is_loopback() {
                    continue;
                }
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
                let mut data = Vec::new();
                let mut buf = [0; 1024];
                while data.len() < 8192 && !data.windows(4).any(|v| v == b"\r\n\r\n") {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => data.extend_from_slice(&buf[..n]),
                    }
                }
                let request = String::from_utf8_lossy(&data);
                let first = request.lines().next().unwrap_or("");
                let parts: Vec<_> = first.split_whitespace().collect();
                let expected_host = format!("host: {host}");
                if parts.len() != 3
                    || parts[0] != "GET"
                    || !request
                        .lines()
                        .any(|line| line.to_ascii_lowercase() == expected_host)
                    || !parts[1].starts_with("/callback?")
                {
                    let _=stream.write_all(b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\nContent-Length: 0\r\n\r\n");
                    continue;
                }
                let result = callback_code(parts[1], &host, &state);
                let body = if result.is_ok() {
                    "Anmeldung empfangen. Du kannst dieses Fenster schliessen und zu Herakles Mail wechseln."
                } else {
                    "Anmeldung fehlgeschlagen. Bitte zu Herakles Mail wechseln."
                };
                let reply=format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nContent-Security-Policy: default-src 'none'\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",body.len());
                let _ = stream.write_all(reply.as_bytes());
                return result;
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100))
            }
            Err(_) => return Err("Anmeldeempfang fehlgeschlagen.".into()),
        }
    }
    Err("Google-Anmeldung nach fünf Minuten abgelaufen.".into())
}
fn same_context(config: &TestConfig) -> Result<(), String> {
    if TestConfig::load()?.cache_key() != config.cache_key() {
        Err("Konto wurde gewechselt. Bitte erneut starten.".into())
    } else {
        Ok(())
    }
}
#[tauri::command]
pub async fn google_connect(
    store: State<'_, Store>,
    gate: State<'_, GoogleGate>,
    sync_gate: State<'_, SyncGate>,
    network: State<'_, NetworkMode>,
) -> Result<(), String> {
    let _guard = gate
        .0
        .try_lock()
        .map_err(|_| "Google-Aktion läuft bereits.")?;
    crate::require_network(&network)?;
    let (client_id, client_secret) = app_client()?;
    let context_guard = sync_gate.0.lock().await;
    crate::require_network(&network)?;
    let config = TestConfig::load()?;
    let initial_context = crate::config::active_context_id();
    let generation = network.0.borrow().generation;
    drop(context_guard);
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|_| "Lokaler Anmeldeempfang konnte nicht gestartet werden.")?;
    let redirect = format!(
        "http://{}/callback",
        listener
            .local_addr()
            .map_err(|_| "Lokaler Anmeldeempfang fehlt.")?
    );
    let verifier = random()?;
    let state = random()?;
    let url = authorization_url(&client_id, &redirect, &state, &verifier);
    open::that(&url).map_err(|_| "Browser konnte nicht geöffnet werden.")?;
    let code = tauri::async_runtime::spawn_blocking(move || await_callback(listener, state))
        .await
        .map_err(|_| "Anmeldung abgebrochen.")??;
    crate::require_network(&network)?;
    same_context(&config)?;
    if initial_context != crate::config::active_context_id()
        || generation != network.0.borrow().generation
    {
        return Err("Anmeldekontext geändert. Bitte erneut anmelden.".into());
    }
    let http = client()?;
    let token = json_response(
        http.post("https://oauth2.googleapis.com/token")
            .form(&[
                ("client_id", client_id.as_str()),
                ("client_secret", client_secret.as_str()),
                ("code", &code),
                ("code_verifier", &verifier),
                ("redirect_uri", &redirect),
                ("grant_type", "authorization_code"),
            ])
            .send()
            .await
            .map_err(|_| "Google-Anmeldung fehlgeschlagen.")?,
    )
    .await?;
    let access = token["access_token"]
        .as_str()
        .ok_or("Google-Zugang fehlt.")?;
    let refresh = token["refresh_token"]
        .as_str()
        .ok_or("Google hat keinen dauerhaften Zugang erteilt. Bitte erneut verbinden.")?;
    let user = json_response(
        http.get("https://openidconnect.googleapis.com/v1/userinfo")
            .bearer_auth(access)
            .send()
            .await
            .map_err(|_| "Google-Konto konnte nicht geprüft werden.")?,
    )
    .await?;
    let subject = user["sub"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() < 512)
        .ok_or("Google-Kontokennung fehlt.")?;
    let email = user["email"]
        .as_str()
        .filter(|s| s.len() < 512)
        .ok_or("Google-Kontoadresse fehlt.")?;
    let _sync = sync_gate.0.lock().await;
    crate::require_network(&network)?;
    same_context(&config)?;
    if initial_context != crate::config::active_context_id()
        || generation != network.0.borrow().generation
    {
        return Err("Anmeldekontext geändert. Bitte erneut anmelden.".into());
    }
    let credential = Credential {
        client_id,
        client_secret,
        refresh_token: refresh.into(),
        subject: subject.into(),
        email: email.into(),
    };
    entry(&config)?
        .set_password(
            &serde_json::to_string(&credential).map_err(|_| "Google-Zugang nicht speicherbar.")?,
        )
        .map_err(|_| "Google-Zugang konnte nicht sicher gespeichert werden.")?;
    // A new account must never inherit another Google account's preview.
    store.put(
        &google_contacts::source_key(&config),
        &serde_json::to_string(&Source {
            subject: subject.into(),
            email: email.into(),
            ..Source::default()
        })
        .unwrap(),
    )?;
    Ok(())
}
#[tauri::command]
pub async fn google_disconnect(
    store: State<'_, Store>,
    gate: State<'_, GoogleGate>,
    sync_gate: State<'_, SyncGate>,
) -> Result<(), String> {
    let _guard = gate.0.lock().await;
    let _sync = sync_gate.0.lock().await;
    let config = TestConfig::load()?;
    match entry(&config)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => {}
        Err(_) => return Err("Google-Zugang konnte nicht entfernt werden.".into()),
    }
    store.put(
        &google_contacts::source_key(&config),
        &serde_json::to_string(&Source::default()).unwrap(),
    )
}
trait PeopleRemote {
    fn page(&self, page: &str) -> impl std::future::Future<Output = Result<Value, String>> + Send;
}
struct People<'a> {
    http: &'a reqwest::Client,
    access: &'a str,
}
impl PeopleRemote for People<'_> {
    async fn page(&self, page: &str) -> Result<Value, String> {
        let mut url =
            url::Url::parse("https://people.googleapis.com/v1/people/me/connections").unwrap();
        url.query_pairs_mut().extend_pairs([
            ("personFields", "names,emailAddresses,phoneNumbers,metadata"),
            ("sources", "READ_SOURCE_TYPE_CONTACT"),
            ("pageSize", "1000"),
        ]);
        if !page.is_empty() {
            url.query_pairs_mut().append_pair("pageToken", page);
        }
        json_response(
            self.http
                .get(url)
                .bearer_auth(self.access)
                .send()
                .await
                .map_err(|_| "Google-Kontakte konnten nicht geladen werden.")?,
        )
        .await
    }
}
async fn collect_people(
    remote: &impl PeopleRemote,
    check: impl Fn() -> Result<(), String>,
) -> Result<BTreeMap<String, Value>, String> {
    let mut cards = BTreeMap::new();
    let mut page = String::new();
    let mut pages = BTreeSet::new();
    let mut bytes = 0;
    loop {
        check()?;
        let value = remote.page(&page).await?;
        if !value.is_object()
            || value.get("error").is_some()
            || value.get("connections").is_some_and(|v| !v.is_array())
            || value.get("nextPageToken").is_some_and(|v| !v.is_string())
        {
            return Err("Ungültige Google-Kontaktseite.".into());
        }
        bytes += serde_json::to_vec(&value)
            .map_err(|_| "Google-Antwort ungültig.")?
            .len();
        if bytes > 32 * 1024 * 1024 {
            return Err("Google-Kontakte überschreiten das Limit von 32 MiB.".into());
        }
        for person in value["connections"].as_array().into_iter().flatten() {
            if person["metadata"]["deleted"] == true {
                continue;
            }
            let card = google_contacts::project(person)?;
            let resource = person["resourceName"]
                .as_str()
                .ok_or("Google-Kennung fehlt.")?;
            if cards.insert(resource.into(), card).is_some() || cards.len() > 100_000 {
                return Err(
                    "Google-Kontaktliste ist zu groß oder enthält doppelte Kennungen.".into(),
                );
            }
        }
        page = value["nextPageToken"].as_str().unwrap_or("").into();
        if page.is_empty() {
            break;
        }
        if !pages.insert(page.clone()) || pages.len() > 1000 {
            return Err("Ungültige Google-Seitenfolge.".into());
        }
    }

    check()?;
    Ok(cards)
}

#[tauri::command]
pub async fn google_fetch_contacts(
    store: State<'_, Store>,
    gate: State<'_, GoogleGate>,
    sync_gate: State<'_, SyncGate>,
    network: State<'_, NetworkMode>,
) -> Result<usize, String> {
    let _guard = gate
        .0
        .try_lock()
        .map_err(|_| "Google-Aktion läuft bereits.")?;
    crate::require_network(&network)?;
    let context_guard = sync_gate.0.lock().await;
    crate::require_network(&network)?;
    let config = TestConfig::load()?;
    let initial_context = crate::config::active_context_id();
    let generation = network.0.borrow().generation;
    let credential = load(&config)?.ok_or("Google zuerst verbinden.")?;
    drop(context_guard);
    let http = client()?;
    let token = json_response(
        http.post("https://oauth2.googleapis.com/token")
            .form(&[
                ("client_id", credential.client_id.as_str()),
                ("client_secret", credential.client_secret.as_str()),
                ("refresh_token", credential.refresh_token.as_str()),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await
            .map_err(|_| "Google-Zugang konnte nicht erneuert werden.")?,
    )
    .await?;
    let access = token["access_token"]
        .as_str()
        .ok_or("Google-Zugang fehlt. Bitte erneut verbinden.")?;
    let cards = collect_people(
        &People {
            http: &http,
            access,
        },
        || {
            crate::require_network(&network)?;
            same_context(&config)?;
            if initial_context != crate::config::active_context_id()
                || generation != network.0.borrow().generation
            {
                return Err("Abrufkontext geändert. Bitte erneut abrufen.".into());
            }
            Ok(())
        },
    )
    .await?;
    let count = cards.len();
    let source = Source {
        subject: credential.subject,
        email: credential.email,
        fetched_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        cards,
    };
    let _sync = sync_gate.0.lock().await;
    crate::require_network(&network)?;
    same_context(&config)?;
    if initial_context != crate::config::active_context_id()
        || generation != network.0.borrow().generation
    {
        return Err("Abrufkontext geändert. Bitte erneut abrufen.".into());
    }
    store.put(
        &google_contacts::source_key(&config),
        &serde_json::to_string(&source)
            .map_err(|_| "Google-Kontakte konnten nicht lokal gespeichert werden.")?,
    )?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oauth_uses_pkce_readonly_scope_and_strict_callback() {
        let url = url::Url::parse(&authorization_url(
            "test.apps.googleusercontent.com",
            "http://127.0.0.1:1234/callback",
            "state",
            "verifier",
        ))
        .unwrap();
        let pairs: BTreeMap<_, _> = url.query_pairs().collect();
        assert_eq!(pairs["scope"], SCOPE);
        assert_eq!(pairs["code_challenge_method"], "S256");
        assert!(!url.as_str().contains("verifier"));
        assert_eq!(
            callback_code("/callback?state=s&code=ok", "127.0.0.1:1234", "s").unwrap(),
            "ok"
        );
        for target in [
            "/callback?state=x&code=ok",
            "/callback?state=s&state=s&code=ok",
            "/callback?state=s&error=access_denied",
            "/other?state=s&code=ok",
        ] {
            assert!(callback_code(target, "127.0.0.1:1234", "s").is_err());
        }
        assert!(validate_client("https://evil.example", "s").is_err());
    }

    #[test]
    fn people_pages_are_bounded_validated_and_never_publish_partial_results() {
        use std::sync::Mutex;
        struct Script(Mutex<std::collections::VecDeque<Result<Value, String>>>);
        impl PeopleRemote for Script {
            async fn page(&self, _page: &str) -> Result<Value, String> {
                self.0.lock().unwrap().pop_front().expect("unexpected page")
            }
        }
        let person =
            serde_json::json!({"resourceName":"people/1","names":[{"displayName":"Alice"}]});
        tauri::async_runtime::block_on(async {
            let good=Script(Mutex::new([Ok(serde_json::json!({"connections":[person.clone()],"nextPageToken":"page2"})),Ok(serde_json::json!({"connections":[{"resourceName":"people/2","phoneNumbers":[{"value":"12345678"}]}]}))].into()));
            assert_eq!(collect_people(&good, || Ok(())).await.unwrap().len(), 2);
            for pages in [
                vec![
                    Ok(serde_json::json!({"connections":[person.clone()],"nextPageToken":"next"})),
                    Err("network".into()),
                ],
                vec![
                    Ok(serde_json::json!({"connections":[person.clone()],"nextPageToken":"next"})),
                    Ok(serde_json::json!({"connections":[person.clone()]})),
                ],
                vec![Ok(serde_json::json!({"connections":{},"nextPageToken":42}))],
                vec![
                    Ok(serde_json::json!({"connections":[],"nextPageToken":"same"})),
                    Ok(serde_json::json!({"nextPageToken":"same"})),
                ],
            ] {
                assert!(collect_people(&Script(Mutex::new(pages.into())), || Ok(()))
                    .await
                    .is_err());
            }
            let stop = Script(Mutex::new([Ok(serde_json::json!({}))].into()));
            assert!(collect_people(&stop, || Err("offline".into()))
                .await
                .is_err());
            assert_eq!(stop.0.lock().unwrap().len(), 1);
        });
    }

    #[test]
    fn google_client_is_app_configuration_not_an_ipc_credential() {
        assert!(app_client_values(None, None).is_err());
        assert!(app_client_values(Some(""), None).is_err());
        assert!(app_client_values(Some("https://invalid.example"), None).is_err());
        let client = app_client_values(
            Some("synthetic.apps.googleusercontent.com"),
            Some("desktop-client"),
        )
        .unwrap();
        assert_eq!(client.0, "synthetic.apps.googleusercontent.com");
        assert_eq!(client.1, "desktop-client");
        assert!(app_client_values(Some("synthetic.apps.googleusercontent.com"), None).is_ok());
    }
}
