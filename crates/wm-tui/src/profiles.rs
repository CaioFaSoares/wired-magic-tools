//! Local profile registry for the TUI.
//!
//! One profile is one enrollment: its own data directory, node identity,
//! agent socket, log and (optionally) display profile. The legacy layout,
//! where the TUI data directory *is* the node data directory, is preserved as
//! the `default` profile so existing installations keep working untouched.
//!
//! Profiles live under `<root>/profiles/<slug>/` and the registry is
//! `<root>/profiles.json` (0600). Each extra profile is a complete agent data
//! directory, which lets several agents run concurrently — each one still
//! needs its own TUN name and UDP port.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use wm_proto::NodeCompliance;

use crate::agent;
use crate::persist;

pub const REGISTRY_FILE: &str = "profiles.json";
pub const PROFILES_DIR: &str = "profiles";
pub const LEGACY_SLUG: &str = "default";
pub const REGISTRY_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileEntry {
    pub slug: String,
    pub label: String,
    /// Relative to the registry root. `None` means the registry root itself
    /// (legacy/default profile).
    #[serde(default)]
    pub root: Option<PathBuf>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Registry {
    pub version: u32,
    pub active: Option<String>,
    pub profiles: Vec<ProfileEntry>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            version: REGISTRY_VERSION,
            active: Some(LEGACY_SLUG.into()),
            profiles: vec![legacy_entry()],
        }
    }
}

fn legacy_entry() -> ProfileEntry {
    ProfileEntry {
        slug: LEGACY_SLUG.into(),
        label: "Padrão".into(),
        root: None,
        created_at: Utc::now(),
    }
}

impl Registry {
    pub fn find(&self, slug: &str) -> Option<&ProfileEntry> {
        self.profiles.iter().find(|entry| entry.slug == slug)
    }

    pub fn root_of(&self, root: &Path, slug: &str) -> Option<PathBuf> {
        self.find(slug).map(|entry| profile_root(root, entry))
    }

    pub fn active_slug(&self) -> &str {
        self.active
            .as_deref()
            .filter(|slug| self.find(slug).is_some())
            .unwrap_or(LEGACY_SLUG)
    }

    pub fn ensure_legacy(&mut self) {
        if !self.profiles.iter().any(|entry| entry.slug == LEGACY_SLUG) {
            self.profiles.insert(0, legacy_entry());
        }
        if self.active.is_none() {
            self.active = Some(LEGACY_SLUG.into());
        }
    }

    pub fn create(&mut self, label: &str, slug: &str) -> Result<ProfileEntry, String> {
        let slug = normalize_slug(slug)?;
        if slug == LEGACY_SLUG {
            return Err("'default' é reservado para o perfil legado".into());
        }
        if self.find(&slug).is_some() {
            return Err(format!("já existe um perfil '{slug}'"));
        }
        let label = label.trim();
        let label = if label.is_empty() { slug.clone() } else { label.to_string() };
        if label.chars().count() > 40 {
            return Err("nome do perfil deve ter no máximo 40 caracteres".into());
        }
        if label.chars().any(char::is_control) {
            return Err("nome do perfil não pode ter caracteres de controle".into());
        }
        let entry = ProfileEntry {
            slug: slug.clone(),
            label,
            root: Some(PathBuf::from(PROFILES_DIR).join(&slug)),
            created_at: Utc::now(),
        };
        self.profiles.push(entry.clone());
        Ok(entry)
    }

    pub fn rename(&mut self, slug: &str, label: &str) -> Result<(), String> {
        let label = label.trim();
        if label.is_empty() || label.chars().count() > 40 {
            return Err("nome do perfil deve ter 1-40 caracteres".into());
        }
        if label.chars().any(char::is_control) {
            return Err("nome do perfil não pode ter caracteres de controle".into());
        }
        let Some(entry) = self.profiles.iter_mut().find(|entry| entry.slug == slug) else {
            return Err(format!("perfil '{slug}' não existe"));
        };
        entry.label = label.to_string();
        Ok(())
    }

    pub fn remove(&mut self, slug: &str) -> Result<(), String> {
        if slug == LEGACY_SLUG {
            return Err("o perfil 'default' não pode ser removido".into());
        }
        let before = self.profiles.len();
        self.profiles.retain(|entry| entry.slug != slug);
        if self.profiles.len() == before {
            return Err(format!("perfil '{slug}' não existe"));
        }
        if self.active.as_deref() == Some(slug) {
            self.active = Some(LEGACY_SLUG.into());
        }
        Ok(())
    }
}

/// Resolves the absolute data directory of a profile, refusing paths that
/// escape the registry root.
pub fn profile_root(registry_root: &Path, entry: &ProfileEntry) -> PathBuf {
    match &entry.root {
        None => registry_root.to_path_buf(),
        Some(relative) => registry_root.join(relative),
    }
}

pub fn registry_path(root: &Path) -> PathBuf {
    root.join(REGISTRY_FILE)
}

pub fn load(root: &Path) -> (Registry, Option<String>) {
    let path = registry_path(root);
    match fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<Registry>(&text) {
            Ok(mut registry) => {
                if registry.version > REGISTRY_VERSION {
                    return (
                        Registry::default(),
                        Some(format!(
                            "registro de perfis de uma versão mais nova (v{}); usando o padrão",
                            registry.version
                        )),
                    );
                }
                registry.version = REGISTRY_VERSION;
                registry.ensure_legacy();
                let before = registry.profiles.len();
                // A hand-edited registry must not point outside `profiles/`.
                registry.profiles.retain(|entry| match &entry.root {
                    None => entry.slug == LEGACY_SLUG,
                    Some(path) => {
                        !path.is_absolute()
                            && path.starts_with(PROFILES_DIR)
                            && path
                                .components()
                                .all(|part| matches!(part, std::path::Component::Normal(_)))
                    }
                });
                let warning = (registry.profiles.len() != before).then(|| {
                    "profiles.json tinha caminhos fora de profiles/; entradas ignoradas".to_string()
                });
                (registry, warning)
            }
            Err(err) => (
                Registry::default(),
                Some(format!("profiles.json ilegível: {err}")),
            ),
        },
        Err(err) if err.kind() == io::ErrorKind::NotFound => (Registry::default(), None),
        Err(err) => (
            Registry::default(),
            Some(format!("não foi possível ler profiles.json: {err}")),
        ),
    }
}

pub fn save(root: &Path, registry: &Registry) -> io::Result<()> {
    fs::create_dir_all(root)?;
    let text = serde_json::to_string_pretty(registry).map_err(io::Error::other)?;
    let path = registry_path(root);
    let tmp = path.with_extension("json.tmp");
    {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        use std::io::Write;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
    }
    fs::rename(&tmp, &path)
}

/// Creates the directory of a new profile. Missing directories are created
/// with 0700 by [`persist::ensure_dir`].
pub fn ensure_profile_dir(root: &Path, entry: &ProfileEntry) -> io::Result<PathBuf> {
    let dir = profile_root(root, entry);
    if entry.root.is_some() {
        let base = root.join(PROFILES_DIR);
        fs::create_dir_all(&base)?;
        persist::ensure_dir(&dir)?;
        let canonical_base = base.canonicalize().unwrap_or(base);
        let canonical_dir = dir.canonicalize().unwrap_or_else(|_| dir.clone());
        if !canonical_dir.starts_with(&canonical_base) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "diretório de perfil fora da raiz permitida",
            ));
        }
    }
    Ok(dir)
}

pub fn normalize_slug(raw: &str) -> Result<String, String> {
    let slug = raw.trim().to_ascii_lowercase().replace(' ', "-");
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() || slug.len() > 32 {
        return Err("identificador do perfil deve ter 1-32 caracteres".into());
    }
    if !slug
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err("identificador do perfil usa letras, números e '-'".into());
    }
    if !slug.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit()) {
        return Err("identificador do perfil deve começar com letra ou número".into());
    }
    Ok(slug)
}

/// One profile as seen by the "Redes" tab.
#[derive(Debug, Clone)]
pub struct ProfileStatus {
    pub slug: String,
    pub label: String,
    pub enrolled: bool,
    pub node_name: String,
    pub display_name: String,
    pub about: String,
    pub overlay_ip: String,
    pub overlay_cidr: String,
    pub control_url: String,
    pub running: bool,
    pub pid: Option<u32>,
    pub node_error: Option<String>,
    pub compliance: Option<NodeCompliance>,
    /// Set when two running profiles would install overlapping overlay routes.
    pub conflict: Option<String>,
}

impl ProfileStatus {
    /// Name to show in panels: the display identity when configured.
    pub fn label_name(&self) -> &str {
        if self.display_name.trim().is_empty() {
            &self.node_name
        } else {
            &self.display_name
        }
    }
}

pub fn refresh(root: &Path, registry: &Registry) -> Vec<ProfileStatus> {
    let mut statuses: Vec<ProfileStatus> = registry
        .profiles
        .iter()
        .map(|entry| status(root, entry))
        .collect();
    mark_conflicts(&mut statuses);
    statuses
}

/// Two agents bound to the same overlay CIDR fight over routes. Flag it before
/// the operator starts the second one instead of debugging silently.
fn mark_conflicts(statuses: &mut [ProfileStatus]) {
    let mut by_cidr: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, status) in statuses.iter().enumerate() {
        if status.running && !status.overlay_cidr.is_empty() {
            by_cidr
                .entry(status.overlay_cidr.clone())
                .or_default()
                .push(index);
        }
    }
    for (cidr, indexes) in by_cidr {
        if indexes.len() < 2 {
            continue;
        }
        for index in indexes {
            let others: Vec<String> = statuses
                .iter()
                .enumerate()
                .filter(|(other, status)| *other != index && status.running && status.overlay_cidr == cidr)
                .map(|(_, status)| status.label.clone())
                .collect();
            statuses[index].conflict = Some(format!(
                "overlay {cidr} também ativo em {}",
                others.join(", ")
            ));
        }
    }
}

pub fn status(root: &Path, entry: &ProfileEntry) -> ProfileStatus {
    let dir = profile_root(root, entry);
    let (node, node_error) = match persist::load_enrolled(&dir) {
        Ok(node) => (node, None),
        Err(err) => (None, Some(err)),
    };
    let compliance = persist::load_state(&dir)
        .0
        .cache
        .and_then(|cache| cache.status)
        .map(|status| status.compliance);
    let socket = dir.join("agent.sock");
    let mut running = agent::socket_ready(&socket);
    let mut pid = agent::read_pid(&dir);
    if !running && pid.is_some_and(agent::pid_alive) && agent::agent_lock_held(&dir) {
        // Socket still booting; the lock is authoritative.
        running = true;
    }
    if !running {
        pid = None;
    }
    let node = node.unwrap_or_default();
    ProfileStatus {
        slug: entry.slug.clone(),
        label: entry.label.clone(),
        enrolled: !node.node_name.is_empty(),
        node_name: node.node_name,
        display_name: node.display_name,
        about: node.about,
        overlay_ip: node.overlay_ip,
        overlay_cidr: node.overlay_cidr,
        control_url: node.control_url,
        running,
        pid,
        node_error,
        compliance,
        conflict: None,
    }
}

/// Starts an agent for a profile that is not the active one. Events are
/// discarded (the TUI only tracks lifecycle for the active profile in detail);
/// the next refresh shows the result.
pub fn start_detached(root: &Path, exe: &Path, slug: &str) -> Result<u32, String> {
    let registry = load(root).0;
    let dir = registry
        .root_of(root, slug)
        .ok_or_else(|| format!("perfil '{slug}' não existe"))?;
    let socket = dir.join("agent.sock");
    if agent::socket_ready(&socket) {
        return Err("o agente deste perfil já está ativo".into());
    }
    if agent::agent_lock_held(&dir) {
        return Err("o agente deste perfil já reservou o data-dir".into());
    }
    if !crate::actions::is_root() {
        return Err("iniciar um agente exige root (reabra com sudo wm tui)".into());
    }
    let settings = persist::load_state(&dir).0.settings;
    let args = agent::args_for_settings(&settings)?;
    let log = persist::agent_log_path(&dir);
    let (tx, rx) = mpsc::channel();
    drop(rx);
    agent::start(&dir, exe, args, &socket, &log, &tx).map_err(|err| err.to_string())
}

/// Stops a profile agent without waiting for the monitor. The "Redes" tab
/// refreshes and shows the new state.
pub fn stop_detached(root: &Path, slug: &str) -> Result<u32, String> {
    let entry = load(root)
        .0
        .find(slug)
        .cloned()
        .ok_or_else(|| format!("perfil '{slug}' não existe"))?;
    let dir = profile_root(root, &entry);
    let socket = dir.join("agent.sock");
    let pid = agent::read_pid(&dir)
        .filter(|pid| agent::pid_alive(*pid))
        .ok_or_else(|| "este perfil não tem agente ativo".to_string())?;
    if !agent::agent_lock_held(&dir) && !agent::socket_ready(&socket) {
        return Err("o pid não corresponde a um agente ativo deste perfil".into());
    }
    agent::signal(pid, libc::SIGTERM).map_err(|err| err.to_string())?;
    Ok(pid)
}

/// True when removing the profile would delete real data; the UI asks for an
/// explicit confirmation in that case.
pub fn has_data(root: &Path, slug: &str) -> bool {
    let Some(entry) = load(root).0.find(slug).cloned() else {
        return false;
    };
    if entry.root.is_none() {
        return true;
    }
    let dir = profile_root(root, &entry);
    dir.join("agent-state.json").exists()
        || dir.join("tui-state.json").exists()
        || dir.join("extensions").exists()
}

pub fn remove_profile(root: &Path, slug: &str) -> Result<(), String> {
    let mut registry = load(root).0;
    let entry = registry
        .find(slug)
        .cloned()
        .ok_or_else(|| format!("perfil '{slug}' não existe"))?;
    if entry.root.is_some() && status(root, &entry).running {
        return Err("pare o agente deste perfil antes de removê-lo".into());
    }
    let dir = profile_root(root, &entry);
    registry.remove(slug)?;
    save(root, &registry).map_err(|err| err.to_string())?;
    if entry.root.is_some() && dir.exists() {
        let base = root.join(PROFILES_DIR).canonicalize().map_err(|err| err.to_string())?;
        let canonical = dir.canonicalize().map_err(|err| err.to_string())?;
        if !canonical.starts_with(&base) {
            return Err("recusando remover um diretório fora da raiz de perfis".into());
        }
        fs::remove_dir_all(&canonical).map_err(|err| err.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("wm");
        persist::ensure_dir(&root).unwrap();
        (dir, root)
    }

    #[test]
    fn legacy_root_is_the_default_profile() {
        let (_dir, root) = root();
        let (registry, warning) = load(&root);
        assert!(warning.is_none());
        assert_eq!(registry.active_slug(), LEGACY_SLUG);
        assert_eq!(registry.root_of(&root, LEGACY_SLUG).unwrap(), root);
    }

    #[test]
    fn creates_distinct_roots_and_persists() {
        let (_dir, root) = root();
        let (mut registry, _) = load(&root);
        let entry = registry.create("Amigos", "Amigos").unwrap();
        assert_eq!(entry.slug, "amigos");
        registry.active = Some(entry.slug.clone());
        save(&root, &registry).unwrap();
        let (loaded, _) = load(&root);
        assert_eq!(loaded.active_slug(), "amigos");
        assert_eq!(
            loaded.root_of(&root, "amigos").unwrap(),
            root.join(PROFILES_DIR).join("amigos")
        );
    }

    #[test]
    fn rejects_reserved_and_duplicate_slugs() {
        let (_dir, root) = root();
        let (mut registry, _) = load(&root);
        assert!(registry.create("x", "default").is_err());
        registry.create("a", "amigos").unwrap();
        assert!(registry.create("b", "amigos").is_err());
        assert!(registry.create("c", "com espaço?".replace(' ', "-").as_str()).is_err());
    }

    #[test]
    fn flags_overlay_conflicts_between_running_profiles() {
        let status = |slug: &str, running: bool| ProfileStatus {
            slug: slug.into(),
            label: slug.into(),
            enrolled: true,
            node_name: slug.into(),
            display_name: String::new(),
            about: String::new(),
            overlay_ip: "10.66.0.2".into(),
            overlay_cidr: "10.66.0.0/24".into(),
            control_url: "http://control".into(),
            running,
            pid: running.then_some(1),
            node_error: None,
            compliance: None,
            conflict: None,
        };
        let mut statuses = vec![status("amigos", true), status("tecnico", true)];
        mark_conflicts(&mut statuses);
        assert!(statuses.iter().all(|status| status.conflict.is_some()));
        let mut stopped = vec![status("amigos", true), status("tecnico", false)];
        mark_conflicts(&mut stopped);
        assert!(stopped.iter().all(|status| status.conflict.is_none()));
    }

    #[test]
    fn reads_identity_from_agent_state() {
        let (_dir, root) = root();
        let (mut registry, _) = load(&root);
        registry.create("Amigos", "amigos").unwrap();
        let entry = registry.find("amigos").unwrap().clone();
        let dir = ensure_profile_dir(&root, &entry).unwrap();
        fs::write(
            dir.join("agent-state.json"),
            serde_json::json!({
                "node_id": "00000000-0000-0000-0000-000000000002",
                "node_token": "wmn_test",
                "overlay_ip": "10.66.0.2",
                "overlay_cidr": "10.66.0.0/24",
                "wg_private_key": "x",
                "wg_public_key": "y",
                "control_url": "http://127.0.0.1:1",
                "node_name": "amigos",
                "display_name": "Caio (amigos)",
                "about": "rack da sala",
                "network_name": "amigos",
                "created_at": "2026-01-01T00:00:00Z"
            })
            .to_string(),
        )
        .unwrap();
        let statuses = refresh(&root, &registry);
        let amigos = statuses.iter().find(|s| s.slug == "amigos").unwrap();
        assert_eq!(amigos.display_name, "Caio (amigos)");
        assert_eq!(amigos.label_name(), "Caio (amigos)");
        assert_eq!(amigos.overlay_cidr, "10.66.0.0/24");
        assert!(!amigos.running);
    }
}
