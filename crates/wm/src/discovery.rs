//! Local extension discovery (`wm debug discover` / `debug-discover`).
//!
//! The debug extension is a diagnostic tool: it scans the local extension
//! directories, parses every `extension.toml` it finds, and reports which ids
//! are new or changed since the last scan. This is intentionally filesystem
//! based — it does not need a control-plane session, and it works for
//! extensions that were dropped into the data dir but not yet loaded by the
//! control plane.
//!
//! Discovery state is a tiny JSON file (by default
//! `<data_dir>/extensions/.debug-discovery.json`) with the first/last time each
//! id was seen. A manifest is reported as `new` for 24h after `first_seen` and
//! as `updated` for 24h after its fingerprint changes. Delete the state file to
//! start over.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use wm_ext::ExtensionManifest;

const STATE_VERSION: u32 = 1;
const MAX_MANIFESTS: usize = 500;
const MAX_DEPTH: usize = 4;

/// Directories that are never extension roots themselves. Skipped while
/// walking so pointing the tool at the repository does not crawl build output.
const IGNORED_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    ".git",
    ".cargo-home",
    ".pnpm-store",
    "web",
    "scripts",
    "docs",
    "deploy",
    "examples",
    "crates",
    "migrations",
];

#[derive(Debug, Clone, Serialize)]
pub struct DiscoverySnapshot {
    pub scanned_at: DateTime<Utc>,
    pub dirs: Vec<String>,
    pub entries: Vec<ExtensionEntry>,
    pub new_count: usize,
    pub updated_count: usize,
    pub warnings: Vec<String>,
}

impl DiscoverySnapshot {
    pub fn empty() -> Self {
        Self {
            scanned_at: Utc::now(),
            dirs: Vec::new(),
            entries: Vec::new(),
            new_count: 0,
            updated_count: 0,
            warnings: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ExtensionEntry {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub visibility: String,
    pub driver: String,
    pub path: String,
    pub status: String,
    pub modified_at: Option<DateTime<Utc>>,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Record {
    fingerprint: String,
    first_seen: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    entries: BTreeMap<String, Record>,
}

pub struct Discovery {
    dirs: Vec<PathBuf>,
    state_file: Option<PathBuf>,
    new_window: Duration,
    state: State,
    load_warnings: Vec<String>,
}

impl Discovery {
    pub fn new(
        dirs: Vec<PathBuf>,
        state_file: Option<PathBuf>,
        new_window: Duration,
    ) -> Self {
        let mut load_warnings = Vec::new();
        let state = match &state_file {
            None => State::default(),
            Some(path) => match fs::read_to_string(path) {
                Ok(text) => match serde_json::from_str::<State>(&text) {
                    Ok(state) => state,
                    Err(err) => {
                        load_warnings.push(format!(
                            "estado de descoberta inválido em {}: {err}; começando do zero",
                            path.display()
                        ));
                        State::default()
                    }
                },
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => State::default(),
                Err(err) => {
                    load_warnings.push(format!(
                        "não foi possível ler {}: {err}",
                        path.display()
                    ));
                    State::default()
                }
            },
        };
        Self {
            dirs,
            state_file,
            new_window,
            state,
            load_warnings,
        }
    }

    pub fn scan(&mut self) -> DiscoverySnapshot {
        let now = Utc::now();
        let mut warnings = std::mem::take(&mut self.load_warnings);
        let mut manifests = Vec::new();

        for dir in &self.dirs {
            if !dir.exists() {
                warnings.push(format!("diretório não encontrado: {}", dir.display()));
                continue;
            }
            collect_manifests(dir, 0, &mut manifests, &mut warnings);
        }
        manifests.sort();
        manifests.dedup();

        let mut entries = Vec::new();
        let mut next_state = BTreeMap::new();
        let mut seen = HashSet::new();
        let mut new_count = 0;
        let mut updated_count = 0;

        for path in manifests.into_iter().take(MAX_MANIFESTS) {
            let dir_id = path
                .parent()
                .and_then(|parent| parent.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "?".into());

            let metadata = fs::metadata(&path).ok();
            let modified_at = metadata
                .as_ref()
                .and_then(|meta| meta.modified().ok())
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .and_then(|since| {
                    DateTime::<Utc>::from_timestamp(
                        since.as_secs() as i64,
                        since.subsec_nanos(),
                    )
                });
            let fingerprint = metadata
                .as_ref()
                .map(|meta| {
                    let modified = meta
                        .modified()
                        .ok()
                        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                        .map(|since| since.as_nanos())
                        .unwrap_or(0);
                    format!("{modified}:{}", meta.len())
                })
                .unwrap_or_else(|| "unknown".into());

            let parsed = parse_manifest(&path, &dir_id);
            let (id, name, version, description, visibility, driver, warning) = match parsed {
                Ok(manifest) => (
                    manifest.id.clone(),
                    manifest.name.clone(),
                    manifest.version.clone(),
                    manifest.description.clone(),
                    manifest.visibility.as_str().to_string(),
                    manifest.node_driver.as_str().to_string(),
                    None,
                ),
                Err(err) => (
                    dir_id.clone(),
                    dir_id.clone(),
                    "—".into(),
                    String::new(),
                    "—".into(),
                    "—".into(),
                    Some(err),
                ),
            };

            if !seen.insert(id.clone()) {
                warnings.push(format!(
                    "id duplicado '{id}' em {}; usando a primeira ocorrência",
                    path.display()
                ));
                continue;
            }

            let status = if warning.is_some() {
                "invalid".to_string()
            } else {
                match self.state.entries.get(&id).cloned() {
                    None => {
                        next_state.insert(
                            id.clone(),
                            Record {
                                fingerprint: fingerprint.clone(),
                                first_seen: now,
                                updated_at: now,
                            },
                        );
                        "new".to_string()
                    }
                    Some(previous) if previous.fingerprint != fingerprint => {
                        next_state.insert(
                            id.clone(),
                            Record {
                                fingerprint: fingerprint.clone(),
                                first_seen: previous.first_seen,
                                updated_at: now,
                            },
                        );
                        "updated".to_string()
                    }
                    Some(previous) => {
                        next_state.insert(id.clone(), previous.clone());
                        if now - previous.first_seen < self.new_window {
                            "new".to_string()
                        } else if now - previous.updated_at < self.new_window {
                            "updated".to_string()
                        } else {
                            "known".to_string()
                        }
                    }
                }
            };

            match status.as_str() {
                "new" => new_count += 1,
                "updated" => updated_count += 1,
                _ => {}
            }

            entries.push(ExtensionEntry {
                id,
                name,
                version,
                description,
                visibility,
                driver,
                path: path.display().to_string(),
                status,
                modified_at,
                warning,
            });
        }

        entries.sort_by(|a, b| a.id.cmp(&b.id));
        self.state = State {
            version: STATE_VERSION,
            entries: next_state,
        };
        self.save(&mut warnings);

        DiscoverySnapshot {
            scanned_at: now,
            dirs: self
                .dirs
                .iter()
                .map(|dir| dir.display().to_string())
                .collect(),
            entries,
            new_count,
            updated_count,
            warnings,
        }
    }

    fn save(&self, warnings: &mut Vec<String>) {
        let Some(path) = &self.state_file else {
            return;
        };
        let Some(parent) = path.parent() else {
            return;
        };
        if let Err(err) = fs::create_dir_all(parent) {
            warnings.push(format!(
                "não foi possível criar {}: {err}",
                parent.display()
            ));
            return;
        }
        let text = match serde_json::to_vec_pretty(&self.state) {
            Ok(text) => text,
            Err(err) => {
                warnings.push(format!("estado de descoberta não serializável: {err}"));
                return;
            }
        };
        let temporary = path.with_extension("json.tmp");
        if let Err(err) = fs::write(&temporary, text) {
            warnings.push(format!(
                "não foi possível gravar {}: {err}",
                temporary.display()
            ));
            return;
        }
        if let Err(err) = fs::rename(&temporary, path) {
            warnings.push(format!(
                "não foi possível substituir {}: {err}",
                path.display()
            ));
            let _ = fs::remove_file(&temporary);
        }
    }
}

fn parse_manifest(path: &Path, dir_id: &str) -> Result<ExtensionManifest, String> {
    let text = fs::read_to_string(path).map_err(|err| format!("não foi possível ler: {err}"))?;
    let manifest: ExtensionManifest =
        toml::from_str(&text).map_err(|err| format!("TOML inválido: {err}"))?;
    manifest.validate().map_err(|err| format!("manifesto inválido: {err}"))?;
    if manifest.id != dir_id {
        return Err(format!(
            "id '{}' não confere com o diretório '{}'",
            manifest.id, dir_id
        ));
    }
    Ok(manifest)
}

pub(crate) fn collect_manifests(
    dir: &Path,
    depth: usize,
    out: &mut Vec<PathBuf>,
    warnings: &mut Vec<String>,
) {
    if depth > MAX_DEPTH || out.len() >= MAX_MANIFESTS {
        return;
    }
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            warnings.push(format!("não foi possível ler {}: {err}", dir.display()));
            return;
        }
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || IGNORED_DIRS.contains(&name.as_str()) {
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
            collect_manifests(&path, depth + 1, out, warnings);
        } else if file_type.is_file() && name == "extension.toml" {
            out.push(path);
        }
    }
}

/// Default roots for the CLI: the runtime data dir and, when run from a
/// checkout, the repository `extensions/` tree.
pub fn default_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![root.join("extensions")];
    let repository = PathBuf::from("extensions");
    if repository.is_dir() {
        dirs.push(repository);
    }
    dirs
}

pub fn default_state_file(root: &Path) -> PathBuf {
    root.join("extensions").join(".debug-discovery.json")
}

/// One-shot CLI used by `wm debug discover` and the `debug-discover` wrapper.
pub fn discover_cli(
    root: &Path,
    dirs: Vec<PathBuf>,
    json: bool,
    no_state: bool,
    window_hours: u64,
) -> anyhow::Result<()> {
    let dirs = if dirs.is_empty() {
        default_dirs(root)
    } else {
        dirs
    };
    let state_file = if no_state {
        None
    } else {
        Some(default_state_file(root))
    };
    let window = Duration::hours(window_hours.max(1) as i64);
    let mut discovery = Discovery::new(dirs, state_file, window);
    let snapshot = discovery.scan();

    if json {
        println!("{}", serde_json::to_string_pretty(&snapshot)?);
        return Ok(());
    }

    if snapshot.entries.is_empty() {
        println!(
            "nenhuma extensão local encontrada em {}",
            snapshot.dirs.join(", ")
        );
    } else {
        println!(
            "{:<24} {:<9} {:<10} {:<9} {}",
            "ID", "VISIB", "DRIVER", "STATUS", "CAMINHO"
        );
        for entry in &snapshot.entries {
            println!(
                "{:<24} {:<9} {:<10} {:<9} {}",
                entry.id,
                entry.visibility,
                entry.driver,
                entry.status,
                entry.path
            );
            if let Some(warning) = &entry.warning {
                println!("  aviso: {warning}");
            }
        }
        println!(
            "\n{} extensão(ões): {} nova(s), {} atualizada(s).",
            snapshot.entries.len(),
            snapshot.new_count,
            snapshot.updated_count
        );
    }
    for warning in &snapshot.warnings {
        eprintln!("aviso: {warning}");
    }
    Ok(())
}

/// Scans the given roots once. Used by the discovery tests.
#[cfg(test)]
pub fn scan_once(
    dirs: Vec<PathBuf>,
    state_file: Option<PathBuf>,
    new_window: Duration,
) -> anyhow::Result<DiscoverySnapshot> {
    let mut discovery = Discovery::new(dirs, state_file, new_window);
    Ok(discovery.scan())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_extension(root: &Path, id: &str, name: &str, visibility: &str) {
        let dir = root.join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("extension.toml"),
            format!(
                "id = \"{id}\"\nname = \"{name}\"\nvisibility = \"{visibility}\"\nnode_driver = \"none\"\n"
            ),
        )
        .unwrap();
    }

    #[test]
    fn finds_manifests_recursively_and_tracks_new_and_updated() {
        let dir = tempfile::tempdir().unwrap();
        let public = dir.path().join("public");
        write_extension(&public, "alpha", "Alpha", "public");
        let state = dir.path().join("state.json");

        let first = scan_once(
            vec![dir.path().to_path_buf()],
            Some(state.clone()),
            Duration::hours(24),
        )
        .unwrap();
        assert_eq!(first.entries.len(), 1);
        assert_eq!(first.entries[0].id, "alpha");
        assert_eq!(first.entries[0].status, "new");
        assert_eq!(first.new_count, 1);
        assert!(state.is_file());

        let second = scan_once(
            vec![dir.path().to_path_buf()],
            Some(state.clone()),
            Duration::hours(24),
        )
        .unwrap();
        assert_eq!(second.entries[0].status, "new");
        assert_eq!(second.new_count, 1);

        std::thread::sleep(std::time::Duration::from_millis(5));
        fs::write(
            public.join("alpha/extension.toml"),
            "id = \"alpha\"\nname = \"Alpha 2\"\nvisibility = \"public\"\nnode_driver = \"none\"\n",
        )
        .unwrap();
        let third = scan_once(
            vec![dir.path().to_path_buf()],
            Some(state),
            Duration::hours(24),
        )
        .unwrap();
        assert_eq!(third.entries[0].status, "updated");
        assert_eq!(third.updated_count, 1);
    }

    #[test]
    fn reports_invalid_manifest_without_losing_the_id() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("broken");
        fs::create_dir_all(&bad).unwrap();
        fs::write(bad.join("extension.toml"), "id = \"broken\"\nname = \"\"\n").unwrap();

        let snapshot = scan_once(
            vec![dir.path().to_path_buf()],
            None,
            Duration::hours(24),
        )
        .unwrap();
        assert_eq!(snapshot.entries.len(), 1);
        assert_eq!(snapshot.entries[0].id, "broken");
        assert_eq!(snapshot.entries[0].status, "invalid");
        assert!(snapshot.entries[0].warning.is_some());
    }

    #[test]
    fn skips_symlinked_directories_and_irrelevant_roots() {
        let dir = tempfile::tempdir().unwrap();
        write_extension(&dir.path().join("ok"), "ok", "OK", "public");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.path(), dir.path().join("loop")).unwrap();
        }
        let snapshot = scan_once(
            vec![dir.path().to_path_buf()],
            None,
            Duration::hours(24),
        )
        .unwrap();
        assert_eq!(snapshot.entries.len(), 1);
        assert_eq!(snapshot.entries[0].id, "ok");
    }

    #[test]
    fn missing_root_is_a_warning_not_a_failure() {
        let snapshot = scan_once(
            vec![PathBuf::from("/nonexistent/wm-discover-test")],
            None,
            Duration::hours(24),
        )
        .unwrap();
        assert!(snapshot.entries.is_empty());
        assert!(
            snapshot
                .warnings
                .iter()
                .any(|warning| warning.contains("não encontrado"))
        );
    }
}
