//! Deployment regions and interface dependencies, independent of any frontend.
use serde::{Deserialize, Serialize};

/// A region describes where core behavior runs, not which UI renders it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionRegion {
    Local,
    Node,
    Server,
}

/// Optional for legacy manifests. New first-party extensions declare their
/// core and CLI before adding a TUI or an optional GUI.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionComponents {
    #[serde(default)]
    pub core: bool,
    #[serde(default)]
    pub cli: bool,
    #[serde(default)]
    pub tui: bool,
    #[serde(default)]
    pub gui: bool,
    #[serde(default)]
    pub regions: Vec<ExtensionRegion>,
    /// Related package providing an optional server (not auto-installed).
    #[serde(default)]
    pub server_extension: Option<String>,
}

impl ExtensionComponents {
    pub fn validate(&self) -> Result<(), String> {
        if !self.core {
            return Err("components.core é obrigatório quando components é declarado".into());
        }
        if !self.cli {
            return Err("components.cli é obrigatório: core → CLI → TUI → GUI opcional".into());
        }
        if self.gui && !self.tui {
            return Err("components.gui requer components.tui".into());
        }
        if self.regions.is_empty() {
            return Err("components.regions deve declarar local, node e/ou server".into());
        }
        for (index, region) in self.regions.iter().enumerate() {
            if self.regions[..index].contains(region) {
                return Err("components.regions contém região duplicada".into());
            }
        }
        if let Some(id) = &self.server_extension {
            if !crate::valid_id(id) {
                return Err("components.server_extension inválido".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interfaces_require_core_and_cli() {
        let mut components = ExtensionComponents::default();
        assert!(components.validate().is_err());
        components.core = true;
        components.regions.push(ExtensionRegion::Local);
        assert!(components.validate().is_err());
        components.cli = true;
        assert!(components.validate().is_ok());
        components.gui = true;
        assert!(components.validate().is_err());
        components.tui = true;
        assert!(components.validate().is_ok());
        components.regions.push(ExtensionRegion::Local);
        assert!(components.validate().is_err());
    }
}
