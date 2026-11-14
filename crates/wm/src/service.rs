//! Direct service execution; no shell. Installation options are explicit, not
//! inferred from a foreground process or transient environment.
use std::{fs::{self, File}, os::fd::AsRawFd, path::{Path, PathBuf}};
use anyhow::{bail, Context};
use clap::{Args, Subcommand, ValueEnum};

#[derive(Clone, Copy, Debug, PartialEq, ValueEnum)]
pub enum Mode { Agent, Host }
impl Mode {
    fn name(self) -> &'static str { match self { Self::Agent => "agent", Self::Host => "host" } }
    fn other(self) -> Self { match self { Self::Agent => Self::Host, Self::Host => Self::Agent } }
    fn id(self) -> String { format!("wired-magic-{}", self.name()) }
    fn label(self) -> String { format!("com.wired-magic.{}", self.name()) }
    fn path(self) -> PathBuf {
        if cfg!(target_os="macos") { PathBuf::from(format!("/Library/LaunchDaemons/{}.plist", self.label())) }
        else { PathBuf::from(format!("/etc/systemd/system/{}.service", self.id())) }
    }
}
#[derive(Args)]
pub struct Install {
    #[arg(long, value_enum, default_value="agent")]
    mode: Mode,
    #[arg(long)] public_url: Option<String>,
    #[command(flatten)] agent: super::AgentArgs,
    #[arg(long)] listen: Option<std::net::SocketAddr>,
    #[arg(long)] tls_cert: Option<PathBuf>,
    #[arg(long)] tls_key: Option<PathBuf>,
    #[arg(long)] no_agent: bool,
    #[arg(long)] approval_required: bool,
}
#[derive(Subcommand)]
pub enum Action {
    Install(Install),
    Uninstall { #[arg(long, value_enum, default_value="agent")] mode: Mode },
    Status { #[arg(long, value_enum, default_value="agent")] mode: Mode },
}

pub struct AgentLock(File);
impl AgentLock {
    /// CLI agent/host modes own machine-wide overlay routes, even with different roots.
    pub fn runtime(root: &Path) -> anyhow::Result<(Option<Self>, Self)> {
        Self::for_operation(root, true)
    }
    pub fn for_operation(root: &Path, starts_agent: bool) -> anyhow::Result<(Option<Self>, Self)> {
        let machine = if starts_agent {
            Some(Self::acquire(Path::new("/var/run/wired-magic"))
                .context("reservando rotas do agente (execute como root)")?)
        } else {
            None
        };
        Ok((machine, Self::acquire(root)?))
    }
    pub fn acquire(root: &Path) -> anyhow::Result<Self> {
        fs::create_dir_all(root)?;
        let file = fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(root.join("agent.lock"))?;
        Self::from_verified_file(file)
    }
    /// Acquire a lock already opened and validated by a privileged caller.
    pub(crate) fn from_verified_file(file: File) -> anyhow::Result<Self> {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            bail!("outro agente/host usa este data-dir; pare-o antes de continuar");
        }
        Ok(Self(file))
    }
}
impl Drop for AgentLock {
    fn drop(&mut self) { unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN); } }
}
fn text(path: &Path) -> anyhow::Result<String> {
    path.to_str().map(str::to_owned).context("caminho do serviço deve ser UTF-8")
}
fn absolute(path: &Path) -> anyhow::Result<PathBuf> {
    Ok(if path.is_absolute() { path.to_owned() } else { std::env::current_dir()?.join(path) })
}
fn argv(exe: &Path, root: &Path, opts: &Install) -> anyhow::Result<Vec<String>> {
    let mut out = vec![text(exe)?, "--data-dir".into(), text(root)?, opts.mode.name().replace("agent", "up")];
    let mut pair = |key: &str, val: String| { out.push(key.into()); out.push(val); };
    pair("--udp-port", opts.agent.udp_port.to_string());
    pair("--mtu", opts.agent.mtu.to_string());
    for value in &opts.agent.expose { pair("--expose", value.clone()); }
    super::parse_expose(&opts.agent.expose)?;
    if opts.mode == Mode::Host {
        if opts.agent.control.is_some() || opts.agent.tun_name.is_some() || opts.agent.force_relay {
            bail!("--control, --tun-name e --force-relay são exclusivos de agent");
        }
        if let Some(url) = &opts.public_url { pair("--public-url", url.clone()); }
        if let Some(addr) = opts.listen { pair("--listen", addr.to_string()); }
        if let Some(path) = &opts.tls_cert { pair("--tls-cert", text(&absolute(path)?)?); }
        if let Some(path) = &opts.tls_key { pair("--tls-key", text(&absolute(path)?)?); }
        if opts.no_agent { out.push("--no-agent".into()); }
        if opts.approval_required { out.push("--approval-required".into()); }
    } else {
        if opts.public_url.is_some() || opts.listen.is_some() || opts.tls_cert.is_some() || opts.tls_key.is_some() || opts.no_agent || opts.approval_required {
            bail!("opções host não são válidas em modo agent; use --control para URL do agente");
        }
        if let Some(url) = &opts.agent.control { pair("--control", url.clone()); }
        if let Some(tun) = &opts.agent.tun_name { pair("--tun-name", tun.clone()); }
        if opts.agent.force_relay { out.push("--force-relay".into()); }
    }
    Ok(out)
}
fn validate(value: &str) -> anyhow::Result<()> {
    if value.chars().any(|c| c.is_control()) { bail!("caractere de controle em argumento do serviço"); }
    Ok(())
}
fn systemd_arg(value: &str) -> anyhow::Result<String> {
    validate(value)?;
    Ok(format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\"").replace('%', "%%").replace('$', "$$")))
}
fn xml(value: &str) -> anyhow::Result<String> {
    validate(value)?;
    Ok(value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;"))
}
fn systemd(args: &[String]) -> anyhow::Result<String> {
    let exec = args.iter().map(|a| systemd_arg(a)).collect::<anyhow::Result<Vec<_>>>()?.join(" ");
    Ok(format!("[Unit]\nDescription=wired-magic\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nType=simple\nExecStart={exec}\nRestart=always\nRestartSec=3\nLimitNOFILE=65536\n\n[Install]\nWantedBy=multi-user.target\n"))
}
fn launchd(mode: Mode, args: &[String]) -> anyhow::Result<String> {
    let args = args.iter().map(|a| Ok(format!("<string>{}</string>", xml(a)?))).collect::<anyhow::Result<Vec<_>>>()?.join("\n");
    Ok(format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n<key>Label</key><string>{}</string>\n<key>ProgramArguments</key><array>{args}</array>\n<key>RunAtLoad</key><true/><key>KeepAlive</key><true/>\n<key>StandardOutPath</key><string>/var/log/{}.log</string>\n<key>StandardErrorPath</key><string>/var/log/{}.log</string>\n</dict></plist>\n", mode.label(), mode.id(), mode.id()))
}
pub fn run(action: Action, root: &Path) -> anyhow::Result<()> {
    match action {
        Action::Status { mode } => status(mode),
        Action::Uninstall { mode } => {
            if cfg!(target_os="macos") { super::run_cmd("launchctl", &["bootout", &format!("system/{}", mode.label())])?; }
            else { super::run_cmd("systemctl", &["disable", "--now", &format!("{}.service", mode.id())])?; }
            fs::remove_file(mode.path())?;
            if !cfg!(target_os="macos") { super::run_cmd("systemctl", &["daemon-reload"])?; }
            Ok(())
        }
        Action::Install(opts) => {
            if unsafe { libc::geteuid() } != 0 { bail!("instale o serviço como root"); }
            let root = absolute(root)?;
            let args = argv(&std::env::current_exe()?, &root, &opts)?;
            let mode = opts.mode;
            // A single machine-wide installation is deliberately conservative:
            // distinct roots may still fight over overlay routes and UDP ports.
            if mode.other().path().exists() { bail!("remova o serviço {} antes de instalar {}", mode.other().name(), mode.name()); }
            let lock = AgentLock::for_operation(&root, mode == Mode::Agent || !opts.no_agent)?;
            let path = mode.path();
            if path.exists() { bail!("serviço já instalado; remova-o antes de reinstalar (inclusive instalações legadas host com ID agent)"); }
            let contents = if cfg!(target_os="macos") { launchd(mode, &args)? } else { systemd(&args)? };
            fs::write(&path, contents)?;
            drop(lock);
            if cfg!(target_os="macos") { super::run_cmd("launchctl", &["bootstrap", "system", &text(&path)?])?; }
            else {
                super::run_cmd("systemctl", &["daemon-reload"])?;
                super::run_cmd("systemctl", &["enable", "--now", &format!("{}.service", mode.id())])?;
            }
            println!("serviço instalado ({})", mode.name());
            Ok(())
        }
    }
}
fn status(mode: Mode) -> anyhow::Result<()> {
    if cfg!(target_os="macos") { super::run_cmd("launchctl", &["print", &format!("system/{}", mode.label())]) }
    else { super::run_cmd("systemctl", &["status", &format!("{}.service", mode.id())]) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    fn options(extra: &[&str]) -> Install {
        let mut args = vec!["wm", "service", "install"];
        args.extend_from_slice(extra);
        match super::super::Cli::try_parse_from(args).unwrap().command {
            super::super::Command::Service(super::super::ServiceArgs { action: Action::Install(opts) }) => opts,
            _ => unreachable!(),
        }
    }
    #[test]
    fn escaping_is_not_shell_execution() {
        let raw = "/tmp/a b'\";&<>$HOME${x}%n\\wm";
        let args = vec![raw.into(), "up".into()];
        let unit = systemd(&args).unwrap();
        assert!(unit.contains("ExecStart=\"/tmp/a b'\\\";&<>$$HOME$${x}%%n\\\\wm\" \"up\""));
        let plist = launchd(Mode::Host, &args).unwrap();
        assert!(plist.contains("<string>/tmp/a b&apos;&quot;;&amp;&lt;&gt;$HOME${x}%n\\wm</string>"));
        assert!(!plist.contains("/bin/sh"));
        assert!(!plist.contains("<string>-c</string>"));
        assert!(plist.contains("com.wired-magic.host"));
    }
    #[test]
    fn generation_preserves_options_and_cli_roundtrips() {
        for mode in ["agent", "host"] {
            let opts = options(&["--mode", mode, "--expose", "8421:8422", "--udp-port", "12345", "--mtu", "1300"]);
            let args = argv(Path::new("/Applications/a b'\"&/wm"), Path::new("/tmp/data a'\"&<>$%"), &opts).unwrap();
            let parsed = super::super::Cli::try_parse_from(&args).unwrap();
            assert_eq!(parsed.data_dir.unwrap(), Path::new("/tmp/data a'\"&<>$%"));
            assert!(args.windows(2).any(|p| p == ["--expose", "8421:8422"]));
            assert!(systemd(&args).is_ok());
            assert!(launchd(opts.mode, &args).is_ok());
        }
    }
    #[test]
    fn urls_host_settings_and_agent_settings_survive() {
        let url = "https://example.test/a?x=1&y=\"'<>$%";
        let opts = options(&["--mode", "host", "--public-url", url, "--listen", "127.0.0.1:9090", "--tls-cert", "/tmp/a b'\"&.pem", "--tls-key", "/tmp/key.pem", "--no-agent", "--approval-required"]);
        let args = argv(Path::new("/wm"), Path::new("/data"), &opts).unwrap();
        let cli = super::super::Cli::try_parse_from(&args).unwrap();
        match cli.command {
            super::super::Command::Host(host) => {
                assert_eq!(host.public_url.as_deref(), Some(url));
                assert_eq!(host.listen.port(), 9090);
                assert!(host.no_agent && host.approval_required);
                assert_eq!(host.tls_cert.unwrap(), Path::new("/tmp/a b'\"&.pem"));
            },
            _ => panic!("expected host"),
        }
        let opts = options(&["--control", url, "--tun-name", "tun42", "--force-relay"]);
        let args = argv(Path::new("/wm"), Path::new("/data"), &opts).unwrap();
        match super::super::Cli::try_parse_from(&args).unwrap().command {
            super::super::Command::Up(agent) => {
                assert_eq!(agent.control.as_deref(), Some(url));
                assert_eq!(agent.tun_name.as_deref(), Some("tun42"));
                assert!(agent.force_relay);
            },
            _ => panic!("expected agent"),
        }
    }
    #[test]
    fn mode_validation_and_control_characters() {
        assert!(super::super::Cli::try_parse_from(["wm", "service", "install", "--mode", "typo"]).is_err());
        assert!(argv(Path::new("/wm"), Path::new("/data"), &options(&["--public-url", "http://host"])).is_err());
        assert!(systemd(&["/tmp/evil\nExecStart=/bad".into()]).is_err());
        assert!(launchd(Mode::Agent, &["bad\0path".into()]).is_err());
        assert_ne!(Mode::Host.path(), Mode::Agent.path());
    }
    #[test]
    fn control_only_and_enrollment_do_not_reserve_machine_routes() {
        let root = std::env::temp_dir().join(format!("wm-cli-control-lock-{}", std::process::id()));
        let (machine, lock) = AgentLock::for_operation(&root, false).unwrap();
        assert!(machine.is_none());
        assert!(AgentLock::acquire(&root).is_err());
        drop(lock);
        assert!(AgentLock::for_operation(&root, false).is_ok());
        for args in [vec!["wm", "host", "--no-agent"], vec!["wm", "join", "--no-start"]] {
            let cli = super::super::Cli::try_parse_from(args).unwrap();
            match cli.command {
                super::super::Command::Host(host) => assert!(host.no_agent),
                super::super::Command::Join(join) => assert!(join.no_start),
                _ => unreachable!(),
            }
        }
    }
    #[test]
    fn lock_blocks_second_instance() {
        let root = std::env::temp_dir().join(format!("wm-cli-lock-{}", std::process::id()));
        let first = AgentLock::acquire(&root).unwrap();
        assert!(AgentLock::acquire(&root).is_err());
        drop(first);
        assert!(AgentLock::acquire(&root).is_ok());
        // Keep the lock inode: unlinking it would introduce a race.
    }
}
