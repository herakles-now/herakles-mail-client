use redb::{Database, ReadableDatabase, TableDefinition};
use std::path::Path;

const SESSIONS: TableDefinition<&str, &str> = TableDefinition::new("session_summaries_v1");

pub struct Store(pub(crate) Database);

impl Store {
    pub fn open(path: &Path) -> Result<Self, String> {
        let database =
            Database::create(path).map_err(|_| "Lokale Datenbank konnte nicht geöffnet werden.")?;
        let transaction = database
            .begin_write()
            .map_err(|_| "Datenbanktransaktion fehlgeschlagen.")?;
        transaction
            .open_table(SESSIONS)
            .map_err(|_| "Datenbankinitialisierung fehlgeschlagen.")?;
        transaction
            .commit()
            .map_err(|_| "Datenbankinitialisierung konnte nicht gespeichert werden.")?;
        let store = Self(database);
        store.initialize_replica()?;
        Ok(store)
    }

    pub fn put(&self, key: &str, summary: &str) -> Result<(), String> {
        let transaction = self
            .0
            .begin_write()
            .map_err(|_| "Datenbanktransaktion fehlgeschlagen.")?;
        {
            let mut table = transaction
                .open_table(SESSIONS)
                .map_err(|_| "Lokaler Speicher nicht verfügbar.")?;
            table
                .insert(key, summary)
                .map_err(|_| "Kontoinformationen konnten nicht gespeichert werden.")?;
        }
        transaction
            .commit()
            .map_err(|_| "Kontoinformationen konnten nicht gespeichert werden.")?;
        Ok(())
    }

    pub fn get(&self, key: &str) -> Result<Option<String>, String> {
        let transaction = self
            .0
            .begin_read()
            .map_err(|_| "Datenbank konnte nicht gelesen werden.")?;
        let table = transaction
            .open_table(SESSIONS)
            .map_err(|_| "Lokaler Speicher nicht verfügbar.")?;
        let value = table
            .get(key)
            .map_err(|_| "Kontoinformationen konnten nicht gelesen werden.")?;
        Ok(value.map(|entry| entry.value().to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_across_reopen_and_separates_accounts() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("test.redb");
        {
            let store = Store::open(&path).unwrap();
            assert_eq!(store.get("account-a").unwrap(), None);
            store.put("account-a", "first").unwrap();
            store.put("account-b", "second").unwrap();
            store.put("account-a", "updated").unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(store.get("account-a").unwrap().as_deref(), Some("updated"));
        assert_eq!(store.get("account-b").unwrap().as_deref(), Some("second"));
    }
}
