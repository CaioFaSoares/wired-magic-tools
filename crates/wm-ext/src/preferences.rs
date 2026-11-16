//! User-selected extension UI preferences shared by CLI and TUI.
//! Loading a preference never activates a node service.
use std::{collections::BTreeMap, fs, io::Write, path::Path};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExtensionPreferences {
    pub favorite: bool,
    pub autoload: bool,
    /// Non-secret presentation options only.
    pub options: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExtensionPreferencesStore {
    pub extensions: BTreeMap<String, ExtensionPreferences>,
}

impl ExtensionPreferencesStore {
    pub fn load(root: &Path) -> Result<Self, String> {
        let path = root.join("extension-preferences.json");
        match fs::read(&path) {
            Ok(bytes) => {
                let store: Self = serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
                store.validate()?;
                Ok(store)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        for (id, prefs) in &self.extensions {
            if !crate::valid_id(id) { return Err(format!("id de extensão inválido: {id}")); }
            for (key, value) in &prefs.options {
                if key.is_empty() || key.len() > 64 || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                    return Err(format!("opção inválida: {key}"));
                }
                if ["token", "secret", "password", "credential", "api_key"].iter().any(|part| key.to_ascii_lowercase().contains(part)) {
                    return Err("opções TUI não podem armazenar credenciais; use o mecanismo de autenticação da extensão".into());
                }
                if value.len() > 4096 || value.chars().any(char::is_control) {
                    return Err(format!("valor inválido para opção {key}"));
                }
            }
        }
        Ok(())
    }

    pub fn save(&self, root: &Path) -> Result<(), String> {
        self.validate()?;
        fs::create_dir_all(root).map_err(|e| e.to_string())?;
        let target = root.join("extension-preferences.json");
        let temp = root.join(format!(".extension-preferences-{}.tmp", std::process::id()));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp).map_err(|e| e.to_string())?;
        let result = (|| {
            file.write_all(&serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
            fs::rename(&temp, &target).map_err(|e| e.to_string())
        })();
        if result.is_err() { let _ = fs::remove_file(&temp); }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preferences_roundtrip_and_profile_isolation() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let mut store = ExtensionPreferencesStore::load(a.path()).unwrap();
        store.extensions.insert("chat".into(), ExtensionPreferences { favorite: true, autoload: true, ..Default::default() });
        store.save(a.path()).unwrap();
        assert_eq!(store, ExtensionPreferencesStore::load(a.path()).unwrap());
        assert!(ExtensionPreferencesStore::load(b.path()).unwrap().extensions.is_empty());
    }
    #[test]
    fn nonsecret_policy_applies_to_every_preferences_writer() {
        let root = tempfile::tempdir().unwrap();
        for key in ["password", "API_KEY", "access_token", "client-secret", "credential"] {
            let mut store = ExtensionPreferencesStore::default();
            store.extensions.entry("chat".into()).or_default().options.insert(key.into(), "must-not-be-saved".into());
            assert!(store.save(root.path()).is_err(), "{key}");
            assert!(!root.path().join("extension-preferences.json").exists());
        }
    }

    #[test]
    fn malformed_preferences_are_not_silently_reset() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("extension-preferences.json"), "bad json").unwrap();
        assert!(ExtensionPreferencesStore::load(root.path()).is_err());
    }
}
