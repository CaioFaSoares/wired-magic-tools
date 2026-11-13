//! Parsing and routing for the in-TUI command line.
//!
//! Every accepted command is an argv vector for this same `wm` binary: there is
//! no shell, no interpolation and no `--data-dir` override. Long-running
//! commands are detached so the TUI can keep rendering.

use std::path::Path;

/// Commands shown by `:` completion, with a short description.
pub const COMMANDS: &[(&str, &str)] = &[
    ("status", "estado do agente"),
    ("peers", "peers do mapa"),
    ("shares", "serviços publicados"),
    ("share", "publicar porta (share <porta> --name <nome>)"),
    ("unshare", "remover share"),
    ("share-rename", "renomear share"),
    ("share-enable", "ligar/desligar share"),
    ("open", "abrir share wm://"),
    ("resolve", "resolver nome da malha"),
    ("ping", "ping no peer"),
    ("probe", "testar porta do peer"),
    ("rename", "renomear este nó"),
    ("up", "iniciar o agente em segundo plano"),
    ("down", "parar o agente"),
    ("join", "entrar na malha"),
    ("host", "subir control plane + relay + nó"),
    ("serve", "subir só o control plane"),
    ("gui", "GUI local no navegador"),
    ("doctor", "diagnóstico da máquina"),
    ("service", "serviço systemd/launchd"),
    ("version", "versão e protocolo"),
    ("help", "atalhos da TUI"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Finishes quickly; output is streamed into the console tab.
    Foreground,
    /// Starts (or joins and starts) the local agent; kept after `q`.
    Agent,
    /// Any other long-running server (`host`, `serve`, `gui`).
    Job(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub args: Vec<String>,
    pub kind: Kind,
}

impl Launch {
    pub fn display(&self) -> String {
        crate::persist::redact_input(&self.args.join(" "))
    }
}

/// Splits a command line into argv, honoring single/double quotes and `\`.
pub fn split(input: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut started = false;
    for ch in input.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            started = true;
            continue;
        }
        match quote {
            Some(delimiter) => {
                if ch == delimiter {
                    quote = None;
                } else if ch == '\\' && delimiter == '"' {
                    escaped = true;
                } else {
                    current.push(ch);
                }
                started = true;
            }
            None => match ch {
                '\\' => {
                    escaped = true;
                    started = true;
                }
                '\'' | '"' => {
                    quote = Some(ch);
                    started = true;
                }
                c if c.is_whitespace() => {
                    if started {
                        args.push(std::mem::take(&mut current));
                        started = false;
                    }
                }
                c => {
                    current.push(c);
                    started = true;
                }
            },
        }
    }
    if escaped {
        return Err("escape '\\' sem caractere depois".into());
    }
    if quote.is_some() {
        return Err("aspas sem fechar".into());
    }
    if started {
        args.push(current);
    }
    Ok(args)
}

fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter()
        .any(|arg| arg == flag || arg.starts_with(&format!("{flag}=")))
}

/// Validates and classifies a command typed by the operator.
pub fn classify(raw: &[String]) -> Result<Launch, String> {
    let mut args: Vec<String> = raw
        .iter()
        .map(|arg| arg.trim().to_string())
        .filter(|arg| !arg.is_empty())
        .collect();
    if args.is_empty() {
        return Err("digite um comando (ex.: status, up, share 3000)".into());
    }
    if args.len() > 64 {
        return Err("comando com argumentos demais".into());
    }
    if args.iter().any(|arg| arg.chars().any(char::is_control)) {
        return Err("caractere de controle no comando".into());
    }
    for arg in &args {
        if arg.len() > 512 {
            return Err("argumento longo demais".into());
        }
        if arg == "--data-dir" || arg.starts_with("--data-dir=") {
            return Err("a TUI fixa o --data-dir; abra outra TUI com --data-dir".into());
        }
    }
    if args[0] == "wm" {
        args.remove(0);
    }
    if args.is_empty() {
        return Err("digite um subcomando depois de 'wm'".into());
    }
    let command = args[0].clone();
    if command.starts_with('-') {
        return Err(format!("'{command}' não é um subcomando"));
    }
    if command == "tui" {
        return Err("a TUI já está aberta; use outro terminal para uma segunda sessão".into());
    }
    let kind = match command.as_str() {
        "up" => Kind::Agent,
        "join" if !has_flag(&args, "--no-start") => Kind::Agent,
        "host" => Kind::Job("host"),
        "serve" => Kind::Job("serve"),
        "gui" => {
            if !has_flag(&args, "--open") && !has_flag(&args, "--no-open") {
                args.push("--open".into());
            }
            Kind::Job("gui")
        }
        _ => Kind::Foreground,
    };
    Ok(Launch { args, kind })
}

/// Runs both normalization steps for the console input.
pub fn parse(input: &str) -> Result<Launch, String> {
    classify(&split(input)?)
}

pub fn suggestions(prefix: &str) -> Vec<&'static str> {
    let prefix = prefix.trim();
    COMMANDS
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| prefix.is_empty() || name.starts_with(prefix))
        .take(8)
        .collect()
}

pub fn description(name: &str) -> Option<&'static str> {
    COMMANDS
        .iter()
        .find(|(command, _)| *command == name)
        .map(|(_, description)| *description)
}

/// Log file under the data dir for a detached job.
pub fn job_log_path(root: &Path, label: &str) -> std::path::PathBuf {
    root.join(format!("{label}.log"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_honors_quotes_and_escapes() {
        assert_eq!(
            split(r#"share 3000 --name "meu app" --description 'a b'"#).unwrap(),
            vec!["share", "3000", "--name", "meu app", "--description", "a b"]
        );
        assert_eq!(split(r#"rename "a \"b\"""#).unwrap(), vec!["rename", "a \"b\""]);
        assert_eq!(split("  ").unwrap(), Vec::<String>::new());
        assert_eq!(split(r#"name "" "#).unwrap(), vec!["name", ""]);
        assert!(split("broken 'quote").is_err());
    }

    #[test]
    fn classify_routes_lifecycle_and_servers() {
        assert_eq!(
            classify(&["up".into()]).unwrap(),
            Launch {
                args: vec!["up".into()],
                kind: Kind::Agent
            }
        );
        assert_eq!(
            classify(&["join".into(), "wmc_x".into()]).unwrap().kind,
            Kind::Agent
        );
        assert_eq!(
            classify(&["join".into(), "wmc_x".into(), "--no-start".into()])
                .unwrap()
                .kind,
            Kind::Foreground
        );
        assert_eq!(classify(&["host".into()]).unwrap().kind, Kind::Job("host"));
        assert_eq!(classify(&["gui".into()]).unwrap().kind, Kind::Job("gui"));
        assert_eq!(
            classify(&["gui".into()]).unwrap().args,
            vec!["gui", "--open"]
        );
        assert!(
            !classify(&["gui".into(), "--no-open".into()])
                .unwrap()
                .args
                .contains(&"--open".to_string())
        );
        assert_eq!(
            classify(&["status".into()]).unwrap().kind,
            Kind::Foreground
        );
    }

    #[test]
    fn classify_rejects_unsafe_input() {
        assert!(classify(&[]).is_err());
        assert!(classify(&["tui".into()]).is_err());
        assert!(classify(&["status".into(), "--data-dir".into(), "/tmp".into()]).is_err());
        assert!(classify(&["status".into(), "--data-dir=/tmp".into()]).is_err());
        assert!(classify(&["status".into(), "bad\nvalue".into()]).is_err());
        assert_eq!(
            classify(&["wm".into(), "status".into()]).unwrap().args,
            vec!["status"]
        );
    }

    #[test]
    fn suggestions_match_prefix_and_describe() {
        assert_eq!(suggestions("sh"), vec!["shares", "share", "share-rename", "share-enable"]);
        assert!(suggestions("").len() <= 8);
        assert!(description("up").unwrap().contains("agente"));
    }
}
