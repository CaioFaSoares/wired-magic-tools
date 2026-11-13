//! Generic `exec` node driver.
//!
//! This is the declarative extension path: an operator drops a manifest with
//! `node_driver = "exec"` and configures a command per node through the panel.
//! The agent runs the command directly (no shell), supervises it and reports
//! its state. It is deliberately small; anything richer should ship its own
//! driver.
//!
//! Trust model: the control-plane admin chooses the command, and the agent runs
//! it as root on the node. Treat extension configuration with the same care as
//! a systemd unit.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use url::Url;

pub const MAX_ARGS: usize = 128;
pub const MAX_ENV: usize = 64;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestartPolicy {
    /// Always restart after the process exits.
    Always,
    /// Restart only on a non-zero exit or a signal.
    #[default]
    OnFailure,
    /// Never restart; the status becomes `stopped` after exit.
    Never,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub working_dir: Option<String>,
    /// Informational URLs shown by the panel. The process itself decides what
    /// it listens on.
    #[serde(default)]
    pub public_urls: Vec<String>,
    #[serde(default)]
    pub restart: RestartPolicy,
}

impl ExecConfig {
    pub fn validate(&self) -> Result<(), String> {
        validate_exec_config(self)
    }
}

pub fn validate_exec_config(config: &ExecConfig) -> Result<(), String> {
    let command = config.command.trim();
    if command.is_empty() || command.chars().count() > 512 {
        return Err("command deve ter 1-512 caracteres".into());
    }
    if command.chars().any(char::is_control) {
        return Err("command contém caracteres de controle".into());
    }
    if config.args.len() > MAX_ARGS {
        return Err(format!("no máximo {MAX_ARGS} argumentos"));
    }
    for arg in &config.args {
        if arg.len() > 4096 || arg.contains('\0') {
            return Err("argumento inválido".into());
        }
    }
    if config.env.len() > MAX_ENV {
        return Err(format!("no máximo {MAX_ENV} variáveis de ambiente"));
    }
    for (key, value) in &config.env {
        if !valid_env_key(key) {
            return Err(format!("nome de variável inválido: {key:?}"));
        }
        if value.len() > 4096 || value.contains('\0') {
            return Err(format!("valor inválido para {key}"));
        }
    }
    if let Some(dir) = &config.working_dir {
        if dir.is_empty() || dir.len() > 1024 || dir.chars().any(char::is_control) {
            return Err("working_dir inválido".into());
        }
    }
    if config.public_urls.len() > 16 {
        return Err("no máximo 16 public_urls".into());
    }
    for raw in &config.public_urls {
        let url = Url::parse(raw).map_err(|_| format!("public_url inválida: {raw}"))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(format!("public_url precisa ser http(s): {raw}"));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> ExecConfig {
        ExecConfig {
            command: "my-sidecar".into(),
            args: vec!["--port".into(), "9000".into()],
            env: BTreeMap::new(),
            working_dir: None,
            public_urls: Vec::new(),
            restart: RestartPolicy::default(),
        }
    }

    #[test]
    fn accepts_a_normal_sidecar() {
        base().validate().unwrap();
        assert_eq!(base().restart, RestartPolicy::OnFailure);
    }

    #[test]
    fn rejects_shell_like_or_empty_commands() {
        let mut cfg = base();
        cfg.command = "".into();
        assert!(cfg.validate().is_err());
        cfg.command = "a\0b".into();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validates_env_keys_and_urls() {
        let mut cfg = base();
        cfg.env.insert("FOO".into(), "bar".into());
        cfg.validate().unwrap();
        cfg.env.insert("1BAD".into(), "x".into());
        assert!(cfg.validate().is_err());
        let mut cfg = base();
        cfg.public_urls = vec!["https://example.com".into()];
        cfg.validate().unwrap();
        cfg.public_urls = vec!["file:///etc/passwd".into()];
        assert!(cfg.validate().is_err());
    }
}
