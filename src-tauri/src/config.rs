use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{OnceLock, RwLock},
};
use url::Url;

const CREDENTIAL_SERVICE: &str = "org.jmapdesktop.client";
const CREDENTIAL_SLOT: &str = "active-account-v1";

#[derive(Clone)]
struct ActiveContext {
    profile_id: String,
    account_id: String,
}
static ACTIVE_CONTEXT: OnceLock<RwLock<Option<ActiveContext>>> = OnceLock::new();

pub fn set_active_context(profile_id: String, account_id: String) {
    let context = ACTIVE_CONTEXT.get_or_init(|| RwLock::new(None));
    if let Ok(mut current) = context.write() {
        *current = Some(ActiveContext {
            profile_id,
            account_id,
        });
    }
}

pub fn clear_active_context() {
    if let Some(context) = ACTIVE_CONTEXT.get() {
        if let Ok(mut current) = context.write() {
            *current = None;
        }
    }
}

pub fn slot_for(profile_id: &str, account_id: &str) -> String {
    if profile_id == "default" && account_id == "legacy" {
        return CREDENTIAL_SLOT.into();
    }
    format!("profile:{profile_id}:account:{account_id}")
}

pub fn active_context_id() -> Option<(String, String)> {
    active_context().map(|context| (context.profile_id, context.account_id))
}

fn active_context() -> Option<ActiveContext> {
    ACTIVE_CONTEXT
        .get()
        .and_then(|context| context.read().ok()?.clone())
}

#[derive(Serialize, Deserialize)]
pub struct TestConfig {
    pub url: Url,
    pub username: String,
    pub password: String,
    pub read_only: bool,
}

impl TestConfig {
    pub fn load() -> Result<Self, String> {
        let context = active_context();
        let slot = context
            .as_ref()
            .map(|context| slot_for(&context.profile_id, &context.account_id))
            .unwrap_or_else(|| CREDENTIAL_SLOT.into());
        let entry = credential_entry(&slot)?;
        match entry.get_password() {
            Ok(value) => {
                let mut config: Self = serde_json::from_str(&value)
                    .map_err(|_| "Gespeicherter Kontozugang ist ungültig.".to_string())?;
                config.url = validate_url(config.url.as_str())?;
                return Ok(config);
            }
            Err(keyring::Error::NoEntry) => {}
            Err(_) => return Err("Der sichere Zugangsspeicher ist nicht verfügbar.".into()),
        }

        if !cfg!(debug_assertions) {
            return Err("Kein Konto eingerichtet. Bitte zuerst ein Konto hinzufügen.".into());
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let path = [root.join(".env.local"), root.join(".env.local.old")]
            .into_iter()
            .find(|path| path.is_file())
            .ok_or_else(|| {
                "Kein Konto eingerichtet. Bitte zuerst ein Konto hinzufügen.".to_string()
            })?;
        let values: HashMap<String, String> = dotenvy::from_path_iter(path)
            .map_err(|_| "Die lokale Testkonfiguration fehlt.".to_string())?
            .collect::<Result<_, _>>()
            .map_err(|_| "Die lokale Testkonfiguration ist ungültig.".to_string())?;
        let required = |key: &str| {
            values
                .get(key)
                .filter(|value| !value.trim().is_empty())
                .cloned()
                .ok_or_else(|| format!("{key} fehlt in .env.local."))
        };
        let config = Self {
            url: validate_url(&required("JMAP_SESSION_URL")?)?,
            username: required("JMAP_USERNAME")?,
            password: required("JMAP_PASSWORD")?,
            read_only: parse_read_only(values.get("JMAP_READ_ONLY").map(String::as_str))?,
        };
        config.save_secure_to(&slot)?;
        Ok(config)
    }

    pub fn from_fields(
        url: &str,
        username: String,
        password: String,
        read_only: bool,
    ) -> Result<Self, String> {
        if username.trim().is_empty() || password.is_empty() {
            return Err("Benutzername und Passwort sind erforderlich.".into());
        }
        Ok(Self {
            url: validate_url(url)?,
            username,
            password,
            read_only,
        })
    }

    pub fn save_secure_to(&self, slot: &str) -> Result<(), String> {
        let entry = credential_entry(slot)?;
        let encoded = serde_json::to_string(self)
            .map_err(|_| "Kontozugang konnte nicht gespeichert werden.")?;
        entry
            .set_password(&encoded)
            .map_err(|_| "Zugang konnte nicht im sicheren Systemspeicher abgelegt werden.")?;
        let saved = entry
            .get_password()
            .map_err(|_| "Gespeicherter Zugang konnte nicht geprüft werden.")?;
        let verified: Self =
            serde_json::from_str(&saved).map_err(|_| "Gespeicherter Zugang ist ungültig.")?;
        if verified.url != self.url
            || verified.username != self.username
            || verified.password != self.password
            || verified.read_only != self.read_only
        {
            return Err(
                "Sicher gespeicherter Zugang stimmt nicht mit der Anmeldung überein.".into(),
            );
        }
        Ok(())
    }

    pub fn cache_key(&self) -> String {
        // Keep the legacy default profile's cache namespace stable during migration.
        if let Some(context) = active_context() {
            if context.profile_id != "default" {
                return serde_json::to_string(&(
                    context.profile_id,
                    self.url.as_str(),
                    &self.username,
                ))
                .expect("string serialization");
            }
        }
        serde_json::to_string(&(self.url.as_str(), &self.username)).expect("string serialization")
    }
}

pub fn delete_secure_slot(slot: &str) {
    if let Ok(entry) = keyring::Entry::new(CREDENTIAL_SERVICE, slot) {
        let _ = entry.delete_credential();
    }
}

fn credential_entry(slot: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(CREDENTIAL_SERVICE, slot)
        .map_err(|_| "Der sichere Zugangsspeicher konnte nicht geöffnet werden.".into())
}

fn parse_read_only(value: Option<&str>) -> Result<bool, String> {
    match value.map(str::trim) {
        None | Some("true") => Ok(true),
        Some("false") => Ok(false),
        _ => Err("JMAP_READ_ONLY muss true oder false sein.".into()),
    }
}

fn validate_url(value: &str) -> Result<Url, String> {
    let url = Url::parse(value).map_err(|_| "JMAP_SESSION_URL ist ungültig.".to_string())?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.query().is_some()
    {
        return Err(
            "JMAP_SESSION_URL muss eine HTTPS-Adresse ohne Zugangsdaten, Query oder Fragment sein."
                .into(),
        );
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_is_default_and_invalid_configuration_never_enables_writes() {
        assert!(parse_read_only(None).unwrap());
        assert!(parse_read_only(Some("true")).unwrap());
        assert!(!parse_read_only(Some("false")).unwrap());
        assert!(parse_read_only(Some("")).is_err());
        assert!(parse_read_only(Some("False")).is_err());
    }

    #[test]
    fn rejects_unsafe_session_urls() {
        for url in [
            "http://mail.example.com/jmap",
            "https://user:secret@mail.example.com/jmap",
            "https://mail.example.com/jmap?token=secret",
            "https://mail.example.com/jmap#fragment",
            "file:///tmp/session",
        ] {
            assert!(validate_url(url).is_err());
        }
        assert!(validate_url("https://mail.example.com/.well-known/jmap").is_ok());
    }
}
