//! Manifest format and discovery.
//!
//! Extensions live in `<data_dir>/extensions/<id>/extension.toml`. Built-in
//! extensions are written to that directory on first boot, so the operator can
//! read and edit the manifest that the panel shows. A manifest declares what the
//! extension needs; code-backed extensions (`cloudflared` today) also have a
//! `ControlExtension` implementation registered by `wm-control`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use wm_proto::{
    ExtensionHooks, HookSpec, MeshExpose, ShareKind, ShareProtocol, validate_share_description,
    validate_share_name,
};

pub const CLOUDFLARED_ID: &str = "cloudflared";
pub const CLOUDFLARED_DRIVER: &str = "cloudflared";
pub const EXEC_DRIVER: &str = "exec";
pub const CHAT_ID: &str = "chat";

/// Distribution class of a bundled extension.
///
/// `Public` extensions ship in the open distribution and are documented as
/// examples. `Private` extensions are kept in the repository (and can be
/// bundled for an internal build) but are not part of the public catalog
/// contract. The class is metadata: it never changes how the runtime loads or
/// supervises an extension.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionVisibility {
    #[default]
    Public,
    Private,
}

impl ExtensionVisibility {
    pub fn as_str(self) -> &'static str {
        match self {
            ExtensionVisibility::Public => "public",
            ExtensionVisibility::Private => "private",
        }
    }
}

/// How a node-side extension is executed by the agent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeDriver {
    /// The extension has no node-side component (control-plane only).
    #[default]
    None,
    /// One generic sidecar process described by [`crate::exec::ExecConfig`].
    Exec,
    /// The built-in cloudflared supervisor.
    Cloudflared,
}

impl NodeDriver {
    pub fn as_str(self) -> &'static str {
        match self {
            NodeDriver::None => "none",
            NodeDriver::Exec => EXEC_DRIVER,
            NodeDriver::Cloudflared => CLOUDFLARED_DRIVER,
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "none" => Some(NodeDriver::None),
            "exec" => Some(NodeDriver::Exec),
            "cloudflared" => Some(NodeDriver::Cloudflared),
            _ => None,
        }
    }
}

/// On-disk manifest. Unknown fields are rejected so a typo does not silently
/// disable a knob the operator believes is active.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExtensionManifest {
    pub id: String,
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub description: String,
    /// Public/private classification used by the catalog and by packaging.
    #[serde(default)]
    pub visibility: ExtensionVisibility,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub homepage: Option<String>,
    /// Set by the loader; a manifest cannot claim to be built in.
    #[serde(default, skip_serializing)]
    pub builtin: bool,
    #[serde(default)]
    pub node_driver: NodeDriver,
    /// Which panel renders this extension: `cloudflared` (custom) or
    /// `generic` (JSON editor). Absent means generic.
    #[serde(default)]
    pub ui: Option<String>,
    /// Whether the extension starts active for a freshly enrolled node. Most
    /// node-side extensions are opt-in; creating a config/route may activate.
    #[serde(default)]
    pub default_active: bool,
    /// Example node config shown by the generic editor. It is documentation,
    /// not a default that is applied automatically.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub config_example: serde_json::Value,
    /// Config materialized when the extension is activated on a node that has
    /// no explicit config yet. Validated by the driver.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub default_config: serde_json::Value,
    /// Lifecycle commands executed on the node.
    #[serde(default)]
    pub hooks: ExtensionHooks,
    /// Declarative effects applied while the extension is active on a node.
    #[serde(default)]
    pub adapter: AdapterSpec,
}

/// Declarative adapter: a manifest can map a product onto mesh primitives
/// instead of asking the operator to do it by hand.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AdapterSpec {
    /// Node labels merged while active. Keys are removed on deactivation when
    /// the current value still matches the adapter value.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub shares: Vec<AdapterShare>,
    #[serde(default)]
    pub exposes: Vec<AdapterExpose>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AdapterShare {
    /// Share name template; `{node}` and `{extension}` are substituted.
    pub name: String,
    pub port: u16,
    #[serde(default)]
    pub kind: ShareKind,
    #[serde(default)]
    pub protocol: ShareProtocol,
    #[serde(default)]
    pub description: String,
}

impl AdapterShare {
    pub fn rendered_name(&self, extension_id: &str, node_name: &str) -> Result<String, String> {
        let raw = self
            .name
            .replace("{extension}", &slug(extension_id))
            .replace("{node}", &slug(node_name));
        validate_share_name(&raw)
    }
}

/// Node names may contain `.`/`_`, which are invalid in share names. Sanitize
/// placeholders instead of rejecting an otherwise valid node.
fn slug(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut dash = false;
    for ch in raw.trim().to_ascii_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "node".into()
    } else {
        out
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AdapterExpose {
    pub overlay_port: u16,
    pub local_port: u16,
}

impl From<AdapterExpose> for MeshExpose {
    fn from(value: AdapterExpose) -> Self {
        Self {
            overlay_port: value.overlay_port,
            local_port: value.local_port,
        }
    }
}

impl AdapterSpec {
    pub fn is_empty(&self) -> bool {
        self.labels.is_empty() && self.shares.is_empty() && self.exposes.is_empty()
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.labels.len() > 32 {
            return Err("adapter.labels: no máximo 32 labels".into());
        }
        for (key, value) in &self.labels {
            if key.trim().is_empty() || key.len() > 64 || key.chars().any(char::is_control) {
                return Err(format!("adapter.labels: chave inválida {key:?}"));
            }
            if value.len() > 256 || value.chars().any(char::is_control) {
                return Err(format!("adapter.labels: valor inválido para {key}"));
            }
        }
        if self.shares.len() > 16 {
            return Err("adapter.shares: no máximo 16 shares".into());
        }
        let mut names = std::collections::HashSet::new();
        for share in &self.shares {
            if share.port == 0 {
                return Err("adapter.shares: porta zero".into());
            }
            validate_share_description(&share.description)
                .map_err(|err| format!("adapter.shares: {err}"))?;
            if !share.name.contains("{node}") && self.shares.len() > 1 {
                if !names.insert(share.name.clone()) {
                    return Err(format!("adapter.shares: nome duplicado '{}'", share.name));
                }
            }
            // Validate both substitutions with harmless sample values.
            share.rendered_name("extension", "node")?;
        }
        if self.exposes.len() > 32 {
            return Err("adapter.exposes: no máximo 32 portas".into());
        }
        let mut ports = std::collections::HashSet::new();
        for expose in &self.exposes {
            if expose.overlay_port == 0 || expose.local_port == 0 {
                return Err("adapter.exposes: porta zero".into());
            }
            if !ports.insert(expose.overlay_port) {
                return Err(format!(
                    "adapter.exposes: porta de overlay {} duplicada",
                    expose.overlay_port
                ));
            }
        }
        Ok(())
    }
}

fn default_version() -> String {
    "0.1.0".into()
}

impl ExtensionManifest {
    pub fn validate(&self) -> Result<(), String> {
        if !valid_id(&self.id) {
            return Err(format!(
                "id inválido '{}': use 1-63 caracteres [a-z0-9_-]",
                self.id
            ));
        }
        if self.name.trim().is_empty() || self.name.chars().count() > 80 {
            return Err("name deve ter 1-80 caracteres".into());
        }
        if self.version.trim().is_empty() || self.version.chars().count() > 32 {
            return Err("version deve ter 1-32 caracteres".into());
        }
        if self.description.chars().count() > 500 {
            return Err("description deve ter no máximo 500 caracteres".into());
        }
        if self.author.chars().count() > 120 {
            return Err("author deve ter no máximo 120 caracteres".into());
        }
        if let Some(ui) = &self.ui {
            if ui.len() > 64 || !ui.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                return Err("ui inválido (alfanumérico/hífen, até 64)".into());
            }
        }
        if self.node_driver == NodeDriver::Cloudflared && self.id != CLOUDFLARED_ID {
            return Err("o driver cloudflared é reservado à extensão 'cloudflared'".into());
        }
        if self.id == CLOUDFLARED_ID && self.node_driver != NodeDriver::Cloudflared {
            return Err("a extensão 'cloudflared' precisa usar node_driver = 'cloudflared'".into());
        }
        validate_hook("install", self.hooks.install.as_ref())?;
        validate_hook("activate", self.hooks.activate.as_ref())?;
        validate_hook("deactivate", self.hooks.deactivate.as_ref())?;
        self.adapter.validate()?;
        if !self.default_config.is_null() {
            if self.node_driver == NodeDriver::Exec {
                let config: crate::ExecConfig = serde_json::from_value(self.default_config.clone())
                    .map_err(|err| format!("default_config inválida para exec: {err}"))?;
                config.validate()?;
            } else if self.node_driver == NodeDriver::Cloudflared {
                return Err("default_config não se aplica ao driver cloudflared".into());
            }
        }
        Ok(())
    }

    pub fn ui_kind(&self) -> &str {
        self.ui.as_deref().unwrap_or("generic")
    }

    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }
}

fn validate_hook(name: &str, hook: Option<&HookSpec>) -> Result<(), String> {
    let Some(hook) = hook else {
        return Ok(());
    };
    let command = hook.command.trim();
    if command.is_empty() || command.chars().count() > 512 {
        return Err(format!("hooks.{name}.command deve ter 1-512 caracteres"));
    }
    if command.chars().any(char::is_control) {
        return Err(format!("hooks.{name}.command contém caracteres de controle"));
    }
    if hook.args.len() > 128 {
        return Err(format!("hooks.{name}: no máximo 128 argumentos"));
    }
    for arg in &hook.args {
        if arg.len() > 4096 || arg.contains('\0') {
            return Err(format!("hooks.{name}: argumento inválido"));
        }
    }
    if hook.env.len() > 64 {
        return Err(format!("hooks.{name}: no máximo 64 variáveis"));
    }
    for (key, value) in &hook.env {
        if !valid_env_key(key) {
            return Err(format!("hooks.{name}: variável inválida {key:?}"));
        }
        if value.len() > 4096 || value.contains('\0') {
            return Err(format!("hooks.{name}: valor inválido para {key}"));
        }
    }
    if hook.timeout_secs == 0 || hook.timeout_secs > 3600 {
        return Err(format!("hooks.{name}.timeout_secs precisa estar em 1..=3600"));
    }
    if let Some(dir) = &hook.working_dir {
        if dir.is_empty() || dir.len() > 1024 || dir.chars().any(char::is_control) {
            return Err(format!("hooks.{name}.working_dir inválido"));
        }
    }
    Ok(())
}

fn valid_env_key(key: &str) -> bool {
    let bytes = key.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 {
        return false;
    }
    if !(bytes[0].is_ascii_alphabetic() || bytes[0] == b'_') {
        return false;
    }
    bytes
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || *b == b'_')
}

pub fn valid_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 63
        && bytes[0].is_ascii_lowercase()
        && bytes.iter().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-' || *b == b'_'
        })
}

pub fn is_builtin_id(id: &str) -> bool {
    id == CLOUDFLARED_ID
}

#[derive(Debug, Default)]
pub struct LoadedManifests {
    pub manifests: Vec<ExtensionManifest>,
    pub warnings: Vec<String>,
}

/// Reads `<root>/<id>/extension.toml` for every directory under `root`.
///
/// A broken or mismatched manifest produces a warning and is skipped: one bad
/// extension must not keep the control plane from booting.
pub fn load_dir(root: &Path) -> LoadedManifests {
    let mut out = LoadedManifests::default();
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return out,
        Err(err) => {
            out.warnings.push(format!(
                "não foi possível ler {}: {err}",
                root.display()
            ));
            return out;
        }
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    for dir in dirs {
        let file = dir.join("extension.toml");
        if !file.is_file() {
            continue;
        }
        let dir_id = dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let text = match fs::read_to_string(&file) {
            Ok(text) => text,
            Err(err) => {
                out.warnings.push(format!("{}: {err}", file.display()));
                continue;
            }
        };
        let mut manifest: ExtensionManifest = match toml::from_str(&text) {
            Ok(manifest) => manifest,
            Err(err) => {
                out.warnings.push(format!("{}: TOML inválido: {err}", file.display()));
                continue;
            }
        };
        if let Err(err) = manifest.validate() {
            out.warnings
                .push(format!("{}: manifesto inválido: {err}", file.display()));
            continue;
        }
        if manifest.id != dir_id {
            out.warnings.push(format!(
                "{}: id '{}' não confere com o diretório '{}'; ignorado",
                file.display(),
                manifest.id,
                dir_id
            ));
            continue;
        }
        if out.manifests.iter().any(|m| m.id == manifest.id) {
            out.warnings
                .push(format!("extensão duplicada '{}'; ignorada", manifest.id));
            continue;
        }
        manifest.builtin = is_builtin_id(&manifest.id);
        out.manifests.push(manifest);
    }
    out
}

// The repository `extensions/` folder is the source of truth for bundled
// extensions. It is split into `public/` (shipped in the open distribution)
// and `private/` (internal/operator-specific). The runtime layout stays flat:
// `<data_dir>/extensions/<id>/`. Everything here is embedded so a distributed
// binary still seeds the manifests and hook scripts.
const CLOUDFLARED_MANIFEST_TOML: &str =
    include_str!("../../../extensions/public/cloudflared/extension.toml");
const CHAT_TUI_MANIFEST_TOML: &str =
    include_str!("../../../extensions/public/chat/extension.toml");
const CHAT_GUI_MANIFEST_TOML: &str =
    include_str!("../../../extensions/public/chat_gui/extension.toml");
const CHAT_GUI_INSTALL_SH: &str =
    include_str!("../../../extensions/public/chat_gui/install.sh");
const CHAT_GUI_ACTIVATE_SH: &str =
    include_str!("../../../extensions/public/chat_gui/activate.sh");
const CHAT_GUI_DEACTIVATE_SH: &str =
    include_str!("../../../extensions/public/chat_gui/deactivate.sh");
const DEBUG_TUI_MANIFEST_TOML: &str =
    include_str!("../../../extensions/public/debug/extension.toml");
const DEBUG_GUI_MANIFEST_TOML: &str =
    include_str!("../../../extensions/public/debug_gui/extension.toml");
const DEBUG_GUI_INSTALL_SH: &str =
    include_str!("../../../extensions/public/debug_gui/install.sh");
const DEBUG_GUI_ACTIVATE_SH: &str =
    include_str!("../../../extensions/public/debug_gui/activate.sh");
const DEBUG_GUI_DEACTIVATE_SH: &str =
    include_str!("../../../extensions/public/debug_gui/deactivate.sh");
const DEBUG_DISCOVER_SH: &str =
    include_str!("../../../extensions/public/debug_gui/debug-discover");
const SEANCE_MANIFEST_TOML: &str =
    include_str!("../../../extensions/private/seance/extension.toml");
const SEANCE_INSTALL_SH: &str = include_str!("../../../extensions/private/seance/install.sh");
const SEANCE_ACTIVATE_SH: &str = include_str!("../../../extensions/private/seance/activate.sh");
const SEANCE_DEACTIVATE_SH: &str = include_str!("../../../extensions/private/seance/deactivate.sh");
const EXTENSIONS_README: &str = include_str!("../../../extensions/README.md");

/// Writes bundled extensions to disk. Existing files are left untouched so
/// operator edits survive upgrades.
pub fn seed_builtin_manifests(root: &Path) -> std::io::Result<()> {
    seed_file(root, "README.md", EXTENSIONS_README, false, false)?;
    seed_file(root, "cloudflared/extension.toml", CLOUDFLARED_MANIFEST_TOML, false, false)?;
    seed_file(root, "chat/extension.toml", CHAT_TUI_MANIFEST_TOML, false, false)?;
    seed_file(root, "chat_gui/extension.toml", CHAT_GUI_MANIFEST_TOML, false, false)?;
    seed_file(root, "debug/extension.toml", DEBUG_TUI_MANIFEST_TOML, false, false)?;
    seed_file(root, "debug_gui/extension.toml", DEBUG_GUI_MANIFEST_TOML, false, false)?;
    seed_file(root, "seance/extension.toml", SEANCE_MANIFEST_TOML, false, false)?;
    // Bundled hook scripts are managed assets: refresh them on boot so an
    // upgrade does not leave an old installer behind. Manifests are not
    // overwritten (operators tune them), and symlinks are never followed.
    seed_file(root, "chat_gui/install.sh", CHAT_GUI_INSTALL_SH, true, true)?;
    seed_file(root, "chat_gui/activate.sh", CHAT_GUI_ACTIVATE_SH, true, true)?;
    seed_file(root, "chat_gui/deactivate.sh", CHAT_GUI_DEACTIVATE_SH, true, true)?;
    seed_file(root, "debug_gui/install.sh", DEBUG_GUI_INSTALL_SH, true, true)?;
    seed_file(root, "debug_gui/activate.sh", DEBUG_GUI_ACTIVATE_SH, true, true)?;
    seed_file(root, "debug_gui/deactivate.sh", DEBUG_GUI_DEACTIVATE_SH, true, true)?;
    seed_file(root, "debug_gui/debug-discover", DEBUG_DISCOVER_SH, true, true)?;
    seed_file(root, "seance/install.sh", SEANCE_INSTALL_SH, true, true)?;
    seed_file(root, "seance/activate.sh", SEANCE_ACTIVATE_SH, true, true)?;
    seed_file(root, "seance/deactivate.sh", SEANCE_DEACTIVATE_SH, true, true)?;
    Ok(())
}

fn seed_file(
    root: &Path,
    relative: &str,
    content: &str,
    executable: bool,
    overwrite: bool,
) -> std::io::Result<()> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    match fs::symlink_metadata(&path) {
        Ok(meta) => {
            if overwrite && meta.file_type().is_file() {
                // A read-only or otherwise unwritable asset must not prevent
                // the agent/control from booting; it will be retried next boot.
                let _ = fs::write(&path, content);
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => fs::write(&path, content)?,
        Err(err) => return Err(err),
    }
    #[cfg(unix)]
    if executable {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::symlink_metadata(&path) {
            if meta.file_type().is_file() {
                let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o755));
            }
        }
    }
    Ok(())
}

pub fn builtin_cloudflared_manifest() -> ExtensionManifest {
    let mut manifest: ExtensionManifest =
        toml::from_str(CLOUDFLARED_MANIFEST_TOML).expect("manifesto cloudflared embutido válido");
    manifest.builtin = true;
    manifest
}

pub fn builtin_chat_tui_manifest() -> ExtensionManifest {
    toml::from_str(CHAT_TUI_MANIFEST_TOML).expect("manifesto chat TUI embutido válido")
}

pub fn builtin_chat_gui_manifest() -> ExtensionManifest {
    toml::from_str(CHAT_GUI_MANIFEST_TOML).expect("manifesto chat_gui embutido válido")
}

pub fn builtin_debug_tui_manifest() -> ExtensionManifest {
    toml::from_str(DEBUG_TUI_MANIFEST_TOML).expect("manifesto debug TUI embutido válido")
}

pub fn builtin_debug_gui_manifest() -> ExtensionManifest {
    toml::from_str(DEBUG_GUI_MANIFEST_TOML).expect("manifesto debug_gui embutido válido")
}

pub fn builtin_seance_manifest() -> ExtensionManifest {
    toml::from_str(SEANCE_MANIFEST_TOML).expect("manifesto seance embutido válido")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_manifest(root: &Path, id: &str, body: &str) {
        let dir = root.join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("extension.toml"), body).unwrap();
    }

    #[test]
    fn loads_and_validates_manifests() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_manifest(
            root,
            "hello",
            r#"
id = "hello"
name = "Hello"
description = "um teste"
node_driver = "exec"
[config_example]
command = "hello"
"#,
        );
        write_manifest(root, "broken", "id = \"broken\"\nname = \"\"\n");
        write_manifest(root, "wrongdir", "id = \"other\"\nname = \"Other\"\n");

        let loaded = load_dir(root);
        assert_eq!(loaded.manifests.len(), 1);
        assert_eq!(loaded.manifests[0].id, "hello");
        assert!(!loaded.manifests[0].builtin);
        assert_eq!(loaded.warnings.len(), 2);
    }

    #[test]
    fn seeds_cloudflared_manifest_once() {
        let dir = tempfile::tempdir().unwrap();
        seed_builtin_manifests(dir.path()).unwrap();
        // Bundled hook scripts are refreshed, not left stale.
        std::fs::write(dir.path().join("seance/install.sh"), "# old").unwrap();
        seed_builtin_manifests(dir.path()).unwrap();
        assert_ne!(
            std::fs::read_to_string(dir.path().join("seance/install.sh")).unwrap(),
            "# old"
        );
        let loaded = load_dir(dir.path());
        assert_eq!(loaded.manifests.len(), 6);
        let m = loaded
            .manifests
            .iter()
            .find(|manifest| manifest.id == CLOUDFLARED_ID)
            .expect("cloudflared");
        assert!(m.builtin);
        assert!(!m.default_active);
        assert_eq!(m.node_driver, NodeDriver::Cloudflared);
        assert_eq!(m.ui_kind(), "cloudflared");
        assert_eq!(m.visibility, ExtensionVisibility::Public);
        let chat = loaded
            .manifests
            .iter()
            .find(|manifest| manifest.id == "chat")
            .expect("chat TUI");
        assert_eq!(chat.node_driver, NodeDriver::None);
        assert_eq!(chat.visibility, ExtensionVisibility::Public);
        assert!(chat.hooks.install.is_none());
        let chat_gui = loaded
            .manifests
            .iter()
            .find(|manifest| manifest.id == "chat_gui")
            .expect("chat_gui");
        assert_eq!(chat_gui.node_driver, NodeDriver::Exec);
        assert_eq!(chat_gui.visibility, ExtensionVisibility::Public);
        assert_eq!(
            chat_gui.default_config["command"],
            "/usr/local/bin/wm-chat-gui"
        );
        assert_eq!(chat_gui.default_config["args"][0], "--listen");
        assert_eq!(chat_gui.adapter.exposes[0].overlay_port, 8790);
        assert_eq!(
            chat_gui.adapter.shares[0]
                .rendered_name("chat_gui", "mac_01.local")
                .unwrap(),
            "chat-gui-mac-01-local"
        );
        assert!(chat_gui.hooks.install.is_some());
        assert!(dir.path().join("chat_gui/install.sh").is_file());
        let debug = loaded
            .manifests
            .iter()
            .find(|manifest| manifest.id == "debug")
            .expect("debug TUI");
        assert_eq!(debug.node_driver, NodeDriver::None);
        assert_eq!(debug.visibility, ExtensionVisibility::Public);
        assert!(debug.hooks.install.is_none());
        let debug_gui = loaded
            .manifests
            .iter()
            .find(|manifest| manifest.id == "debug_gui")
            .expect("debug_gui");
        assert_eq!(debug_gui.node_driver, NodeDriver::Exec);
        assert_eq!(debug_gui.visibility, ExtensionVisibility::Public);
        assert_eq!(
            debug_gui.default_config["command"],
            "/usr/local/bin/wm-debug-gui"
        );
        assert_eq!(debug_gui.default_config["args"][0], "--listen");
        assert_eq!(debug_gui.adapter.exposes[0].overlay_port, 8791);
        assert_eq!(
            debug_gui.adapter.shares[0]
                .rendered_name("debug_gui", "mac_01.local")
                .unwrap(),
            "debug-gui-mac-01-local"
        );
        assert!(debug_gui.hooks.install.is_some());
        assert!(dir.path().join("debug_gui/install.sh").is_file());
        assert!(dir.path().join("debug_gui/debug-discover").is_file());
        let seance = loaded
            .manifests
            .iter()
            .find(|manifest| manifest.id == "seance")
            .expect("seance");
        assert_eq!(seance.node_driver, NodeDriver::Exec);
        assert_eq!(seance.visibility, ExtensionVisibility::Private);
        assert!(seance.default_config["command"].as_str().is_some());
        assert_eq!(seance.adapter.exposes[0].overlay_port, 8421);
        assert_eq!(
            seance.adapter.shares[0]
                .rendered_name("seance", "mac_01.local")
                .unwrap(),
            "baguette-mac-01-local"
        );
        assert!(seance.hooks.install.is_some());
        assert!(dir.path().join("seance/install.sh").is_file());
    }

    #[test]
    fn rejects_reserved_driver_for_other_id() {
        let dir = tempfile::tempdir().unwrap();
        write_manifest(
            dir.path(),
            "sneaky",
            "id = \"sneaky\"\nname = \"Sneaky\"\nnode_driver = \"cloudflared\"\n",
        );
        let loaded = load_dir(dir.path());
        assert!(loaded.manifests.is_empty());
        assert_eq!(loaded.warnings.len(), 1);
    }

    #[test]
    fn shipped_example_manifest_parses() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/extensions");
        let loaded = load_dir(&root);
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        let hello = loaded
            .manifests
            .iter()
            .find(|manifest| manifest.id == "hello")
            .expect("hello example");
        assert_eq!(hello.node_driver, NodeDriver::Exec);
        assert_eq!(hello.config_example["command"], "/usr/local/bin/hello");
    }

    #[test]
    fn id_rules_are_strict() {
        assert!(valid_id("cloudflared"));
        assert!(valid_id("my-ext-2"));
        assert!(valid_id("chat_gui"));
        assert!(!valid_id("MyExt"));
        assert!(!valid_id("-nope"));
        assert!(!valid_id("with space"));
        assert!(!valid_id(""));
    }
}
