//! Local extension catalog for the TUI.
//!
//! The control-plane catalog is fetched from the agent and only exists while
//! the node is live. This module scans `<data_dir>/extensions` directly, so the
//! operator can see what is installed/available even when the agent is down,
//! and marks entries as new/updated using the same `.debug-discovery.json`
//! state used by `debug-discover`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;

const MAX_DEPTH: usize = 2;
const IGNORED: &[&str] = &[".git", ".sources", "node_modules", "target"];

#[derive(Debug, Clone, Default)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub version: String,
    pub visibility: String,
    pub driver: String,
    pub status: String,
    pub path: String,
    pub source: String,
    pub commit: String,
}

#[derive(Debug, Default, Deserialize)]
struct Manifest {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    visibility: Option<String>,
    #[serde(default)]
    node_driver: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct DiscoveryState {
    #[serde(default)]
    entries: BTreeMap<String, DiscoveryRecord>,
}

#[derive(Debug, Deserialize)]
struct DiscoveryRecord {
    first_seen: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Default, Deserialize)]
struct PackagesLock {
    #[serde(default)]
    packages: BTreeMap<String, PackageRecord>,
}

#[derive(Debug, Deserialize)]
struct PackageRecord {
    #[serde(default)]
    source: String,
    #[serde(default)]
    commit: String,
}

#[derive(Debug, Default)]
pub struct Catalog {
    pub entries: Vec<Entry>,
    pub error: Option<String>,
}

/// Scans the local extension directory and enriches entries with package and
/// discovery state. Errors are collected (never propagated) so the TUI keeps
/// rendering.
pub fn scan(root: &Path) -> Catalog {
    let extensions = root.join("extensions");
    let mut catalog = Catalog::default();
    if !extensions.is_dir() {
        catalog.error = Some(format!("{} não encontrado", extensions.display()));
        return catalog;
    }

    let discovery = read_json::<DiscoveryState>(&extensions.join(".debug-discovery.json"));
    let packages = read_json::<PackagesLock>(&extensions.join(".packages.json"));

    let mut manifests = Vec::new();
    collect(&extensions, 0, &mut manifests);
    manifests.sort();

    for path in manifests {
        let file = path.join("extension.toml");
        let text = match fs::read_to_string(&file) {
            Ok(text) => text,
            Err(err) => {
                catalog.error = Some(format!("{}: {err}", file.display()));
                continue;
            }
        };
        let manifest: Manifest = match toml::from_str(&text) {
            Ok(manifest) => manifest,
            Err(err) => {
                catalog.error = Some(format!("{}: TOML inválido: {err}", file.display()));
                continue;
            }
        };
        if manifest.id.trim().is_empty() {
            continue;
        }
        let package = packages.packages.get(&manifest.id);
        let status = catalog_status(discovery.entries.get(&manifest.id));
        let path = path
            .strip_prefix(&extensions)
            .unwrap_or(&path)
            .display()
            .to_string();
        catalog.entries.push(Entry {
            id: manifest.id,
            name: if manifest.name.trim().is_empty() {
                "?".into()
            } else {
                manifest.name
            },
            version: manifest.version,
            visibility: manifest.visibility.unwrap_or_else(|| "public".into()),
            driver: manifest.node_driver.unwrap_or_else(|| "none".into()),
            status,
            path,
            source: package.map(|package| package.source.clone()).unwrap_or_default(),
            commit: package.map(|package| package.commit.clone()).unwrap_or_default(),
        });
    }
    catalog.entries.sort_by(|a, b| a.id.cmp(&b.id));
    catalog
}

fn catalog_status(record: Option<&DiscoveryRecord>) -> String {
    let Some(record) = record else {
        return "conhecida".into();
    };
    let now = Utc::now();
    let window = Duration::hours(24);
    if now - record.first_seen < window {
        "nova".into()
    } else if now - record.updated_at < window {
        "atualizada".into()
    } else {
        "ok".into()
    }
}

fn collect(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || IGNORED.contains(&name.as_str()) {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            if path.join("extension.toml").is_file() {
                out.push(path);
            } else {
                collect(&path, depth + 1, out);
            }
        }
    }
}

fn read_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_flat_and_nested_manifests() {
        let dir = tempfile::tempdir().unwrap();
        let extensions = dir.path().join("extensions");
        fs::create_dir_all(extensions.join("chat_gui")).unwrap();
        fs::write(
            extensions.join("chat_gui/extension.toml"),
            "id = \"chat_gui\"\nname = \"Chat GUI\"\nversion = \"0.1.0\"\nnode_driver = \"exec\"\nvisibility = \"public\"\n",
        )
        .unwrap();
        fs::create_dir_all(extensions.join("private/seance")).unwrap();
        fs::write(
            extensions.join("private/seance/extension.toml"),
            "id = \"seance\"\nname = \"Seance\"\nversion = \"0.1.0\"\nnode_driver = \"exec\"\nvisibility = \"private\"\n",
        )
        .unwrap();

        let catalog = scan(dir.path());
        assert_eq!(catalog.entries.len(), 2);
        assert_eq!(catalog.entries[0].id, "chat_gui");
        assert_eq!(catalog.entries[0].driver, "exec");
        assert_eq!(catalog.entries[1].visibility, "private");
    }

    #[test]
    fn missing_extensions_dir_is_a_visible_error() {
        let catalog = scan(Path::new("/nonexistent/wm-tui-catalog-test"));
        assert!(catalog.entries.is_empty());
        assert!(catalog.error.is_some());
    }

    #[test]
    fn debug_state_marks_recent_entries() {
        let dir = tempfile::tempdir().unwrap();
        let extensions = dir.path().join("extensions");
        fs::create_dir_all(extensions.join("chat")).unwrap();
        fs::write(
            extensions.join("chat/extension.toml"),
            "id = \"chat\"\nname = \"Chat\"\nversion = \"0.1.0\"\nnode_driver = \"none\"\n",
        )
        .unwrap();
        let now = Utc::now();
        fs::write(
            extensions.join(".debug-discovery.json"),
            serde_json::json!({
                "entries": {
                    "chat": {
                        "first_seen": now,
                        "updated_at": now
                    }
                }
            })
            .to_string(),
        )
        .unwrap();
        let catalog = scan(dir.path());
        assert_eq!(catalog.entries[0].status, "nova");
    }
}
