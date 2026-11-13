//! Manifest format and discovery.
//!
//! Extensions live in `<data_dir>/extensions/<id>/extension.toml`. Built-in
//! extensions are written to that directory on first boot, so the operator can
//! read and edit the manifest that the panel shows. A manifest declares what the
//! extension needs; code-backed extensions (`cloudflared` today) also have a
//! `ControlExtension` implementation registered by `wm-control`.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const CLOUDFLARED_ID: &str = "cloudflared";
pub const CLOUDFLARED_DRIVER: &str = "cloudflared";
pub const EXEC_DRIVER: &str = "exec";

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
    /// Example node config shown by the generic editor. It is documentation,
    /// not a default that is applied automatically.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub config_example: serde_json::Value,
}

fn default_version() -> String {
    "0.1.0".into()
}

impl ExtensionManifest {
    pub fn validate(&self) -> Result<(), String> {
        if !valid_id(&self.id) {
            return Err(format!(
                "id inválido '{}': use 1-63 caracteres [a-z0-9-]",
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
        Ok(())
    }

    pub fn ui_kind(&self) -> &str {
        self.ui.as_deref().unwrap_or("generic")
    }

    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }
}

pub fn valid_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 63
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
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

/// Writes the built-in manifests to disk. Existing files are left untouched so
/// operator edits survive upgrades.
pub fn seed_builtin_manifests(root: &Path) -> std::io::Result<()> {
    fs::create_dir_all(root)?;
    let dir = root.join(CLOUDFLARED_ID);
    fs::create_dir_all(&dir)?;
    let file = dir.join("extension.toml");
    if !file.exists() {
        let text = builtin_cloudflared_manifest()
            .to_toml()
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        fs::write(&file, text)?;
    }
    Ok(())
}

pub fn builtin_cloudflared_manifest() -> ExtensionManifest {
    ExtensionManifest {
        id: CLOUDFLARED_ID.into(),
        name: "Cloudflare Tunnel".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        description: "Expõe serviços locais de um nó pela Cloudflare, com rotas por projeto e URL pública gerenciadas pelo painel.".into(),
        author: "wired-magic".into(),
        homepage: Some("https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/".into()),
        builtin: true,
        node_driver: NodeDriver::Cloudflared,
        ui: Some("cloudflared".into()),
        config_example: serde_json::Value::Null,
    }
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
        seed_builtin_manifests(dir.path()).unwrap();
        let loaded = load_dir(dir.path());
        assert_eq!(loaded.manifests.len(), 1);
        let m = &loaded.manifests[0];
        assert_eq!(m.id, CLOUDFLARED_ID);
        assert!(m.builtin);
        assert_eq!(m.node_driver, NodeDriver::Cloudflared);
        assert_eq!(m.ui_kind(), "cloudflared");
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
        assert!(!valid_id("MyExt"));
        assert!(!valid_id("-nope"));
        assert!(!valid_id("with space"));
        assert!(!valid_id(""));
    }
}
