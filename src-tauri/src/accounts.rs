use crate::{
    config::{self, TestConfig},
    session,
    storage::Store,
};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

const REGISTRY_KEY: &str = "profile-registry-v1";

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountRecord {
    pub id: String,
    pub label: String,
    pub username: String,
    pub server_url: String,
    pub read_only: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileRecord {
    pub id: String,
    pub name: String,
    pub accounts: Vec<AccountRecord>,
    pub active_account_id: Option<String>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Registry {
    pub profiles: Vec<ProfileRecord>,
    pub active_profile_id: Option<String>,
}

fn read(store: &Store) -> Result<Registry, String> {
    store
        .get(REGISTRY_KEY)?
        .map(|value| {
            serde_json::from_str(&value)
                .map_err(|_| "Lokale Profilverwaltung ist ungültig.".to_string())
        })
        .transpose()
        .map(|registry| registry.unwrap_or_default())
}

fn write(store: &Store, registry: &Registry) -> Result<(), String> {
    let value = serde_json::to_string(registry)
        .map_err(|_| "Profilverwaltung konnte nicht gespeichert werden.")?;
    store.put(REGISTRY_KEY, &value)
}

fn slot(profile_id: &str, account_id: &str) -> String {
    config::slot_for(profile_id, account_id)
}

fn account_id(username: &str, server_url: &str) -> Result<String, String> {
    let url = url::Url::parse(server_url).map_err(|_| "JMAP-Adresse ist ungültig.")?;
    let host = url.host_str().ok_or("JMAP-Adresse enthält keinen Host.")?;
    Ok(format!("{username}@{host}"))
}

fn new_profile_id() -> String {
    let ticks = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("profile-{ticks:x}")
}

fn active_context(registry: &Registry) -> Option<(&ProfileRecord, &AccountRecord)> {
    let profile = registry
        .profiles
        .iter()
        .find(|profile| Some(&profile.id) == registry.active_profile_id.as_ref())?;
    let account = profile
        .accounts
        .iter()
        .find(|account| Some(&account.id) == profile.active_account_id.as_ref())?;
    Some((profile, account))
}

/// Move the existing single-account keyring entry into a default profile without
/// changing its legacy redb cache namespace.
pub fn initialize(store: &Store) -> Result<(), String> {
    let mut registry = read(store)?;
    if registry.profiles.is_empty() {
        config::set_active_context("default".into(), "legacy".into());
        let legacy = match TestConfig::load() {
            Ok(config) => config,
            Err(error) if error.starts_with("Kein Konto eingerichtet.") => return Ok(()),
            Err(error) => return Err(error),
        };
        let profile = ProfileRecord {
            id: "default".into(),
            name: "Privat".into(),
            accounts: Vec::new(),
            active_account_id: None,
        };
        let id = account_id(&legacy.username, legacy.url.as_str())?;
        legacy.save_secure_to(&slot(&profile.id, &id))?;
        let account = AccountRecord {
            id: id.clone(),
            label: legacy.username.clone(),
            username: legacy.username.clone(),
            server_url: legacy.url.to_string(),
            read_only: legacy.read_only,
        };
        registry.profiles.push(ProfileRecord {
            accounts: vec![account],
            active_account_id: Some(id.clone()),
            ..profile
        });
        registry.active_profile_id = Some("default".into());
        write(store, &registry)?;
        config::set_active_context("default".into(), id);
        config::delete_secure_slot("active-account-v1");
        return Ok(());
    }
    if let Some(profile) = registry
        .profiles
        .iter()
        .find(|profile| Some(&profile.id) == registry.active_profile_id.as_ref())
    {
        if profile.accounts.is_empty() {
            config::clear_active_context();
            write(store, &registry)?;
            return Ok(());
        }
    }
    let selected = active_context(&registry)
        .or_else(|| {
            registry
                .profiles
                .iter()
                .find(|profile| Some(&profile.id) == registry.active_profile_id.as_ref())
                .and_then(|profile| profile.accounts.first().map(|account| (profile, account)))
        })
        .map(|(profile, account)| (profile.id.clone(), account.id.clone()))
        .or_else(|| {
            registry.profiles.first().and_then(|profile| {
                profile
                    .accounts
                    .first()
                    .map(|account| (profile.id.clone(), account.id.clone()))
            })
        });
    let Some((profile_id, account_id)) = selected else {
        config::clear_active_context();
        return Ok(());
    };
    registry.active_profile_id = Some(profile_id.clone());
    config::set_active_context(profile_id, account_id);
    let _ = TestConfig::load()?;
    write(store, &registry)
}

pub fn inventory(store: &Store) -> Result<Registry, String> {
    read(store)
}

pub fn create_profile(store: &Store, name: String) -> Result<Registry, String> {
    let name = name.trim();
    if name.is_empty() || name.len() > 64 {
        return Err("Profilname muss 1 bis 64 Zeichen enthalten.".into());
    }
    let mut registry = read(store)?;
    let id = new_profile_id();
    registry.profiles.push(ProfileRecord {
        id: id.clone(),
        name: name.into(),
        accounts: Vec::new(),
        active_account_id: None,
    });
    registry.active_profile_id = Some(id);
    write(store, &registry)?;
    Ok(registry)
}

pub async fn add_account(
    store: &Store,
    profile_id: Option<String>,
    profile_name: Option<String>,
    label: Option<String>,
    server_url: String,
    username: String,
    password: String,
    read_only: bool,
) -> Result<Registry, String> {
    let mut registry = read(store)?;
    let mut profile = if let Some(id) = profile_id {
        registry
            .profiles
            .iter()
            .find(|profile| profile.id == id)
            .cloned()
            .ok_or("Profil wurde nicht gefunden.")?
    } else {
        let name = profile_name.unwrap_or_else(|| "Privat".into());
        let name = name.trim();
        if name.is_empty() || name.len() > 64 {
            return Err("Profilname muss 1 bis 64 Zeichen enthalten.".into());
        }
        ProfileRecord {
            id: if registry.profiles.is_empty() {
                "default".into()
            } else {
                new_profile_id()
            },
            name: name.into(),
            accounts: Vec::new(),
            active_account_id: None,
        }
    };
    let id = account_id(&username, &server_url)?;
    if profile.accounts.iter().any(|account| account.id == id) {
        return Err("Dieses Konto ist in diesem Profil bereits vorhanden.".into());
    }

    // Do not save a credential until the authenticated JMAP session has been tested.
    let config = TestConfig::from_fields(&server_url, username.clone(), password, read_only)?;
    let _session = session::probe(&config).await?;
    let credential_slot = slot(&profile.id, &id);
    config.save_secure_to(&credential_slot)?;
    let account = AccountRecord {
        id: id.clone(),
        label: label
            .filter(|label| !label.trim().is_empty())
            .unwrap_or_else(|| username.clone()),
        username,
        server_url: config.url.to_string(),
        read_only,
    };
    profile.accounts.push(account);
    profile.active_account_id = Some(id.clone());
    if let Some(existing) = registry
        .profiles
        .iter_mut()
        .find(|existing| existing.id == profile.id)
    {
        *existing = profile.clone();
    } else {
        registry.profiles.push(profile.clone());
    }
    registry.active_profile_id = Some(profile.id.clone());
    if let Err(error) = write(store, &registry) {
        config::delete_secure_slot(&credential_slot);
        return Err(error);
    }
    config::set_active_context(profile.id, id);
    Ok(registry)
}

pub fn activate_profile(store: &Store, profile_id: &str) -> Result<Registry, String> {
    let mut registry = read(store)?;
    let profile = registry
        .profiles
        .iter()
        .find(|profile| profile.id == profile_id)
        .ok_or("Profil wurde nicht gefunden.")?;
    let account = profile
        .accounts
        .iter()
        .find(|account| Some(&account.id) == profile.active_account_id.as_ref())
        .or_else(|| profile.accounts.first())
        .cloned();
    registry.active_profile_id = Some(profile_id.into());
    write(store, &registry)?;
    if let Some(account) = account {
        config::set_active_context(profile_id.into(), account.id);
        let _ = TestConfig::load()?;
    } else {
        config::clear_active_context();
    }
    Ok(registry)
}

pub fn activate_account(
    store: &Store,
    profile_id: &str,
    account_id: &str,
) -> Result<Registry, String> {
    let mut registry = read(store)?;
    let profile = registry
        .profiles
        .iter_mut()
        .find(|profile| profile.id == profile_id)
        .ok_or("Profil wurde nicht gefunden.")?;
    if !profile
        .accounts
        .iter()
        .any(|account| account.id == account_id)
    {
        return Err("Konto wurde in diesem Profil nicht gefunden.".into());
    }
    profile.active_account_id = Some(account_id.into());
    registry.active_profile_id = Some(profile_id.into());
    let old = config::active_context_id();
    config::set_active_context(profile_id.into(), account_id.into());
    if let Err(error) = TestConfig::load() {
        if let Some((old_profile, old_account)) = old {
            config::set_active_context(old_profile, old_account);
        }
        return Err(error);
    }
    write(store, &registry)?;
    Ok(registry)
}
