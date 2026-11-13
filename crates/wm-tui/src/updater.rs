//! Homebrew owns replacement/checksum verification; never overwrite a running binary.
use crate::{Msg, app::Level};
use anyhow::{Context, bail};
use serde_json::Value;
use std::os::unix::{fs::OpenOptionsExt, process::CommandExt};
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc::Sender,
    time::{Duration, Instant},
};

pub(super) fn start(tx: Sender<Msg>) {
    if std::env::var_os("WM_NO_AUTO_UPDATE").is_some_and(|v| v == "1") {
        return;
    }
    std::thread::spawn(move || {
        if let Err(error) = update(&tx) {
            let _ = tx.send(Msg::Notify(
                Level::Warn,
                format!(
                    "Atualização automática indisponível: {error:#}. A sessão continua normalmente."
                ),
            ));
        }
    });
}

fn notify(tx: &Sender<Msg>, level: Level, text: impl Into<String>) {
    let _ = tx.send(Msg::Notify(level, text.into()));
}

/// Only manage the formula that actually owns this executable (not a PATH namesake).
fn installation(exe: &Path) -> Option<(PathBuf, PathBuf)> {
    if exe.file_name()? != "wm" || exe.parent()?.file_name()? != "bin" {
        return None;
    }
    let keg = exe.parent()?.parent()?;
    let rack = keg.parent()?;
    if rack.file_name()? != "wired-magic" || rack.parent()?.file_name()? != "Cellar" {
        return None;
    }
    Some((rack.parent()?.parent()?.to_owned(), keg.to_owned()))
}

fn formula(receipt: &Value) -> anyhow::Result<String> {
    let tap = receipt
        .pointer("/source/tap")
        .and_then(Value::as_str)
        .context("receipt sem tap de origem")?;
    let parts: Vec<_> = tap.split('/').collect();
    if parts.len() != 2
        || parts.iter().any(|s| {
            s.is_empty()
                || s.starts_with('.')
                || s.starts_with('-')
                || !s
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
        })
    {
        bail!("tap de origem inválido");
    }
    Ok(format!("{tap}/wired-magic"))
}

fn newer(candidate: &str, current: &str) -> bool {
    match (
        semver::Version::parse(candidate),
        semver::Version::parse(current),
    ) {
        (Ok(next), Ok(now)) => next.pre.is_empty() && next > now,
        _ => false,
    }
}

fn update(tx: &Sender<Msg>) -> anyhow::Result<()> {
    let exe = std::env::current_exe()?.canonicalize()?;
    let Some((prefix, keg)) = installation(&exe) else {
        return Ok(());
    };
    let receipt: Value = serde_json::from_slice(&fs::read(keg.join("INSTALL_RECEIPT.json"))?)?;
    let formula = formula(&receipt)?;
    if unsafe { libc::geteuid() } == 0 {
        notify(
            tx,
            Level::Info,
            format!(
                "Atualizações Homebrew: execute `brew update && brew upgrade {formula}` como usuário normal (sem sudo). A TUI e o agente não serão interrompidos."
            ),
        );
        return Ok(());
    }
    let brew = prefix.join("bin/brew");
    let locks = prefix.join("var/homebrew/locks");
    fs::create_dir_all(&locks)?;
    // Keep the lock inode: unlinking after unlock would race other TUI sessions.
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(locks.join("wired-magic-tui-update.lock"))?;
    use std::os::fd::AsRawFd;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Ok(());
    }
    notify(
        tx,
        Level::Info,
        "Verificando atualizações Homebrew em segundo plano…",
    );
    run(&brew, &["update"], Duration::from_secs(120))?;
    let info: Value = serde_json::from_slice(&run(
        &brew,
        &["info", "--json=v2", &formula],
        Duration::from_secs(30),
    )?)?;
    let entry = info
        .pointer("/formulae/0")
        .context("formula não encontrada")?;
    let full_name = entry
        .get("full_name")
        .and_then(Value::as_str)
        .context("formula sem full_name")?;
    if !full_name.eq_ignore_ascii_case(&formula) {
        bail!("origem da fórmula não confere");
    }
    let latest = entry
        .pointer("/versions/stable")
        .and_then(Value::as_str)
        .context("versão estável ausente")?;
    if !newer(latest, env!("CARGO_PKG_VERSION")) {
        return Ok(());
    }
    if entry.get("pinned").and_then(Value::as_bool) == Some(true) {
        notify(
            tx,
            Level::Warn,
            format!(
                "Versão {latest} disponível; fórmula fixada (brew pin). Atualização automática respeitou o pin."
            ),
        );
        return Ok(());
    }
    // An earlier session may already have installed the latest keg.
    let already_installed = entry
        .get("installed")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items
                .iter()
                .any(|item| item.get("version").and_then(Value::as_str) == Some(latest))
        });
    if !already_installed {
        notify(
            tx,
            Level::Info,
            format!(
                "Nova versão {latest}: baixando e atualizando via Homebrew sem interromper esta sessão…"
            ),
        );
        run(&brew, &["upgrade", &formula], Duration::from_secs(600))?;
    }
    let installed: Value = serde_json::from_slice(&run(
        &brew,
        &["info", "--json=v2", &formula],
        Duration::from_secs(30),
    )?)?;
    if !installed
        .pointer("/formulae/0/installed")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items
                .iter()
                .any(|item| item.get("version").and_then(Value::as_str) == Some(latest))
        })
    {
        bail!("Homebrew não confirmou a instalação de {latest}");
    }
    notify(
        tx,
        Level::Good,
        format!(
            "Versão {latest} instalada! Será usada na próxima abertura. TUI e agente atuais continuam ativos; nenhum reinício foi forçado."
        ),
    );
    Ok(())
}

/// No inherited terminal, shell, password prompts or unbounded pipe buffers.
fn run(brew: &Path, args: &[&str], timeout: Duration) -> anyhow::Result<Vec<u8>> {
    let path = std::env::temp_dir().join(format!("wm-brew-{}.log", uuid::Uuid::new_v4()));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    // Unlink our verified, freshly created path; the open descriptors retain it.
    fs::remove_file(&path)?;
    let mut child = Command::new(brew)
        .args(args)
        .env("HOMEBREW_NO_INSTALL_CLEANUP", "1")
        .env("HOMEBREW_NO_AUTO_UPDATE", "1")
        .env("HOMEBREW_NO_ENV_HINTS", "1")
        .env("CI", "1")
        .stdin(Stdio::null())
        .stdout(file.try_clone()?)
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .context("iniciar Homebrew")?;
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                bail!(
                    "brew {} falhou ({status}); tente manualmente fora da TUI",
                    args.join(" ")
                );
            }
            use std::io::{Read, Seek, SeekFrom};
            let mut file: File = file;
            file.seek(SeekFrom::Start(0))?;
            let mut bytes = Vec::new();
            file.take(2 * 1024 * 1024).read_to_end(&mut bytes)?;
            return Ok(bytes);
        }
        if started.elapsed() >= timeout {
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.wait();
            bail!("tempo limite em brew {}", args.join(" "));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stable_versions_only_and_no_downgrades() {
        assert!(newer("0.2.0", "0.1.0"));
        for version in ["0.1.0", "0.0.9", "0.2.0-rc.1", "garbage"] {
            assert!(!newer(version, "0.1.0"));
        }
        assert!(newer("1.0.0", "1.0.0-rc.1"));
    }
    #[test]
    fn only_own_homebrew_keg_is_managed() {
        assert!(installation(Path::new("/opt/homebrew/Cellar/wired-magic/0.1.0/bin/wm")).is_some());
        for path in [
            "/usr/local/bin/wm",
            "/tmp/Cellar/other/1/bin/wm",
            "/tmp/wm",
            "/opt/homebrew/Cellar/wired-magic/1/bin/other",
        ] {
            assert!(installation(Path::new(path)).is_none());
        }
    }
    #[test]
    fn subprocess_is_noninteractive_and_bounded() {
        // /bin/sh stands in for brew; no real package changes in tests.
        let output = run(
            Path::new("/bin/sh"),
            &["-c", "printf '{\"ok\":true}'"],
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(output, b"{\"ok\":true}");
        assert!(
            run(
                Path::new("/bin/sh"),
                &["-c", "exit 3"],
                Duration::from_secs(2)
            )
            .is_err()
        );
        assert!(
            run(
                Path::new("/bin/sh"),
                &["-c", "sleep 10"],
                Duration::from_millis(10)
            )
            .is_err()
        );
        let output = run(
            Path::new("/bin/sh"),
            &[
                "-c",
                "printf '%s' \"$HOMEBREW_NO_INSTALL_CLEANUP:$CI\"; read answer || true",
            ],
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(output, b"1:1");
    }
    #[test]
    fn validates_receipt_origin() {
        assert_eq!(
            formula(&serde_json::json!({"source":{"tap":"CaioFaSoares/tap"}})).unwrap(),
            "CaioFaSoares/tap/wired-magic"
        );
        for tap in [
            "",
            "-x/tap",
            ".hidden/tap",
            "a/b/c",
            "a/../b",
            "a/.",
            "a/..",
            "a/b;evil",
        ] {
            assert!(formula(&serde_json::json!({"source":{"tap":tap}})).is_err());
        }
    }
}
