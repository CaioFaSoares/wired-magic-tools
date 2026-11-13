use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpStream, UnixStream};
use wm_control::auth;
use wm_control::config::ControlConfig;
use wm_control::db;
use wm_core::AgentConfig;
use wm_proto::{
    CONNECT_CODE_PREFIX, DEFAULT_MTU, DEFAULT_UDP_PORT, LocalRequest, LocalResponse, ShareInfo,
    ShareKind, ShareProtocol, decode_connect_code, decode_join_target, encode_connect_code,
};

mod gui;
mod service;

#[derive(Parser)]
#[command(name = "wm", version, about = "wired-magic — malha VPN self-hosted")]
struct Cli {
    /// Diretório de estado do agente (ou raiz do host).
    #[arg(long, global = true, env = "WM_DATA_DIR")]
    data_dir: Option<PathBuf>,
    /// Nível de log (trace, debug, info, warn, error).
    #[arg(long, global = true, default_value = "info", env = "WM_LOG")]
    log: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Sobe o control plane, o relay/STUN e o nó local em um comando.
    Host(HostArgs),
    /// Sobe apenas o control plane (API + relay + STUN).
    Serve(ServeArgs),
    /// Entra na malha e inicia o agente.
    Join(JoinArgs),
    /// Inicia o agente já inscrito.
    Up(AgentArgs),
    /// Para o agente em execução.
    Down,
    /// Mostra o estado do nó local.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Lista os peers vistos pelo agente.
    Peers {
        #[arg(long)]
        json: bool,
    },
    /// Ping ICMP pelo overlay.
    Ping { peer: String },
    /// Testa uma porta TCP em um nó pelo overlay.
    Probe { peer: String, port: u16 },
    /// Resolve um nome da malha.
    Resolve { name: String },
    /// Publica um serviço local na malha com um nome.
    Share(ShareArgs),
    /// Lista os serviços publicados na malha.
    Shares {
        #[arg(long)]
        json: bool,
    },
    /// Remove um share deste nó.
    Unshare { name: String },
    /// Renomeia um share deste nó.
    ShareRename { name: String, new_name: String },
    /// Liga ou desliga um share deste nó.
    ShareEnable {
        name: String,
        /// Desliga em vez de ligar.
        #[arg(long)]
        off: bool,
    },
    /// Renomeia este nó na malha.
    Rename { name: String },
    /// Abre um share web ou imprime o endereço de um jogo/TCP.
    Open {
        #[arg(value_name = "NOME|wm://NOME")]
        target: String,
        /// Só imprime, sem abrir o navegador.
        #[arg(long)]
        print: bool,
    },
    /// Abre a GUI local de compartilhamento.
    Gui(GuiArgs),
    /// Lista e ativa/desativa extensões neste nó.
    Extensions {
        #[arg(long)]
        json: bool,
        #[command(subcommand)]
        action: Option<ExtensionsAction>,
    },
    /// Abre a TUI (o agente iniciado por ela continua rodando ao sair).
    Tui(TuiArgs),
    /// Diagnóstico da máquina e do nó.
    Doctor,
    /// Instala/remove o serviço do agente.
    Service(ServiceArgs),
    /// Mostra versão e protocolo.
    Version,
}

#[derive(clap::Args)]
struct HostArgs {
    #[arg(long, env = "WM_PUBLIC_URL")]
    public_url: Option<String>,
    /// Endereço HTTP/API/relay (use --public-url para anunciar a porta pública).
    #[arg(long, env = "WM_LISTEN", default_value = "127.0.0.1:8080")]
    listen: SocketAddr,
    #[command(flatten)]
    stun: StunArgs,
    #[arg(long)]
    tls_cert: Option<PathBuf>,
    #[arg(long)]
    tls_key: Option<PathBuf>,
    #[arg(long)]
    admin_password: Option<String>,
    #[arg(long)]
    approval_required: bool,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    no_agent: bool,
    #[arg(long, env = "WM_UDP_PORT", default_value_t = DEFAULT_UDP_PORT)]
    udp_port: u16,
    #[arg(long, default_value_t = DEFAULT_MTU)]
    mtu: u16,
    #[arg(long = "tag")]
    tags: Vec<String>,
    #[arg(long = "label")]
    labels: Vec<String>,
    #[arg(long = "expose")]
    expose: Vec<String>,
}

/// Portas STUN de escuta e anúncio podem diferir atrás de NAT/proxy UDP.
#[derive(clap::Args)]
struct StunArgs {
    /// Endereço UDP do servidor STUN (padrão: 0.0.0.0:3478).
    #[arg(long, env = "WM_STUN_LISTEN")]
    stun_listen: Option<SocketAddr>,
    /// URL STUN pública (stun:host:porta); repetível, ou lista separada por vírgulas no env.
    #[arg(long = "stun-url", env = "WM_STUN_URLS", value_delimiter = ',')]
    stun_urls: Vec<String>,
}

impl StunArgs {
    fn apply(self, cfg: &mut ControlConfig) {
        if let Some(listen) = self.stun_listen {
            cfg.stun_listen = listen;
        }
        if !self.stun_urls.is_empty() {
            cfg.stun_urls = self.stun_urls;
        }
    }
}

#[derive(clap::Args)]
struct ServeArgs {
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long, env = "WM_LISTEN")]
    listen: Option<SocketAddr>,
    #[command(flatten)]
    stun: StunArgs,
    #[arg(long)]
    public_url: Option<String>,
    #[arg(long)]
    tls_cert: Option<PathBuf>,
    #[arg(long)]
    tls_key: Option<PathBuf>,
}

#[derive(clap::Args)]
struct JoinArgs {
    /// URL do control plane ou um código wmc_...
    url: Option<String>,
    #[arg(long)]
    token: Option<String>,
    /// Código opaco wmc_... (contém URL + convite).
    #[arg(long)]
    code: Option<String>,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    no_start: bool,
    #[arg(long)]
    force: bool,
    #[arg(long = "tag")]
    tags: Vec<String>,
    #[arg(long = "label")]
    labels: Vec<String>,
    #[arg(long, default_value_t = DEFAULT_UDP_PORT)]
    udp_port: u16,
    #[arg(long, default_value_t = DEFAULT_MTU)]
    mtu: u16,
    #[arg(long)]
    tun_name: Option<String>,
    /// Não tenta caminho direto; usa sempre o relay.
    #[arg(long)]
    force_relay: bool,
    /// Expõe um serviço loopback no overlay: 8421 ou 8421:8422 (overlay:local).
    #[arg(long = "expose")]
    expose: Vec<String>,
}

#[derive(clap::Args)]
struct ShareArgs {
    /// Porta local publicada (ex.: 3000).
    port: u16,
    /// Nome curto usado em wm://nome.
    #[arg(long)]
    name: Option<String>,
    /// web, game, tcp ou udp.
    #[arg(long, default_value = "web")]
    kind: String,
    /// tcp, udp ou both (padrão depende do tipo).
    #[arg(long)]
    protocol: Option<String>,
    #[arg(long, default_value = "")]
    description: String,
    /// Mantém o share desligado ao criar.
    #[arg(long)]
    disabled: bool,
}

#[derive(Subcommand)]
enum ExtensionsAction {
    /// Ativa a extensão neste nó.
    Enable { id: String },
    /// Desativa a extensão neste nó.
    Disable { id: String },
}

#[derive(clap::Args)]
struct GuiArgs {
    #[arg(long, default_value = "127.0.0.1:8787")]
    listen: SocketAddr,
    /// Não tenta abrir o navegador.
    #[arg(long)]
    no_open: bool,
}

#[derive(clap::Args)]
struct AgentArgs {
    #[arg(long, default_value_t = DEFAULT_UDP_PORT)]
    udp_port: u16,
    #[arg(long, default_value_t = DEFAULT_MTU)]
    mtu: u16,
    #[arg(long)]
    tun_name: Option<String>,
    #[arg(long)]
    control: Option<String>,
    /// Não tenta caminho direto; usa sempre o relay.
    #[arg(long)]
    force_relay: bool,
    /// Expõe um serviço loopback no overlay: 8421 ou 8421:8422 (overlay:local).
    #[arg(long = "expose")]
    expose: Vec<String>,
}

#[derive(clap::Args)]
struct TuiArgs {
    /// Não inicia o agente automaticamente ao abrir a TUI.
    #[arg(long)]
    no_auto_start: bool,
    /// Não captura o mouse (mantém a seleção de texto nativa do terminal).
    #[arg(long)]
    no_mouse: bool,
}

#[derive(clap::Args)]
struct ServiceArgs {
    #[command(subcommand)]
    action: service::Action,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    init_tracing(&cli.log);
    if let Err(err) = run(cli).await {
        eprintln!("erro: {err:#}");
        std::process::exit(1);
    }
}

fn init_tracing(level: &str) {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}

fn data_root(cli: &Cli) -> PathBuf {
    cli.data_dir
        .clone()
        .unwrap_or_else(wm_core::state::default_data_dir)
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    let root = data_root(&cli);
    match cli.command {
        Command::Host(args) => run_host(args, root).await,
        Command::Serve(args) => run_serve(args, root).await,
        Command::Join(args) => run_join(args, root).await,
        Command::Up(args) => run_up(args, root).await,
        Command::Down => run_down(&root).await,
        Command::Status { json } => run_status(&root, json).await,
        Command::Peers { json } => run_peers(&root, json).await,
        Command::Ping { peer } => run_local(&root, "ping", json!({ "peer": peer })).await,
        Command::Probe { peer, port } => {
            run_local(&root, "probe", json!({ "peer": peer, "port": port })).await
        }
        Command::Resolve { name } => run_local(&root, "resolve", json!({ "name": name })).await,
        Command::Share(args) => run_share(&root, args).await,
        Command::Shares { json } => run_shares(&root, json).await,
        Command::Unshare { name } => run_unshare(&root, &name).await,
        Command::ShareRename { name, new_name } => {
            run_share_rename(&root, &name, &new_name).await
        }
        Command::ShareEnable { name, off } => run_share_toggle(&root, &name, !off).await,
        Command::Rename { name } => run_rename(&root, &name).await,
        Command::Open { target, print } => run_open(&root, &target, print).await,
        Command::Gui(args) => gui::run(root, args.listen, !args.no_open).await,
        Command::Extensions { json, action } => run_extensions(&root, json, action).await,
        Command::Tui(args) => run_tui(&root, args).await,
        Command::Doctor => run_doctor(&root).await,
        Command::Service(args) => run_service(args, &root).await,
        Command::Version => {
            println!(
                "wm {} (protocolo {})",
                env!("CARGO_PKG_VERSION"),
                wm_proto::PROTOCOL_VERSION
            );
            Ok(())
        }
    }
}

async fn run_host(args: HostArgs, root: PathBuf) -> anyhow::Result<()> {
    let _instance = service::AgentLock::for_operation(&root, !args.no_agent)?;
    let control_dir = root.join("control");
    let agent_dir = root.clone();
    let public_url = args.public_url.clone().unwrap_or_else(|| {
        let port = args.listen.port();
        format!("http://127.0.0.1:{port}")
    });
    let mut cfg = ControlConfig {
        listen: args.listen,
        public_url: public_url.clone(),
        tls_cert: args.tls_cert.clone(),
        tls_key: args.tls_key.clone(),
        data_dir: control_dir.clone(),
        approval_required: args.approval_required,
        admin_password: args.admin_password.clone(),
        ..Default::default()
    };
    cfg.apply_env();
    // clap resolves CLI > environment > default for the listener.
    cfg.listen = args.listen;
    args.stun.apply(&mut cfg);
    cfg.validate()?;

    let listen = cfg.listen;
    let local_control_url = host_control_url(listen, cfg.tls_cert.is_some(), &public_url);
    let cfg_for_server = cfg.clone();
    let server = tokio::spawn(async move {
        if let Err(err) = wm_control::server::run(cfg_for_server).await {
            eprintln!("control plane parou: {err:#}");
        }
    });
    wait_for_port(listen).await?;

    let pool = db::open(&cfg.db_path()).await?;
    let admin = db::admin_by_username(&pool, "admin")
        .await?
        .ok_or_else(|| anyhow::anyhow!("admin não encontrado após subir o control"))?;
    let actor = format!("admin:{}", admin.username);

    let agent_cfg = AgentConfig {
        data_dir: agent_dir.clone(),
        // The colocated agent must not depend on proxy readiness/health routing.
        control_url: Some(local_control_url.clone()),
        udp_port: args.udp_port,
        mtu: args.mtu,
        tun_name: None,
        name: args.name.clone(),
        tags: {
            let mut tags = args.tags.clone();
            if !tags.iter().any(|t| t == "gateway") {
                tags.push("gateway".into());
            }
            tags
        },
        labels: parse_labels(&args.labels)?,
        force_relay: false,
        expose: parse_expose(&args.expose)?,
    };

    let mut agent_state = match wm_core::NodeState::load(&agent_cfg.state_path())? {
        Some(state) => state,
        None => {
            let host_token = mint_join_token(&pool, &actor, "host-bootstrap", 1, 3600, &agent_cfg.tags).await?;
            println!("inscrevendo o nó local como gateway...");
            let state = wm_core::join(&agent_cfg, &local_control_url, &host_token, args.name.as_deref())
                .await
                .context("inscrevendo o nó local")?;
            println!("  nó: {}  overlay: {}", state.node_name, state.overlay_ip);
            state
        }
    };

    // Persist the public identity for invitations/GUI; transport for this host's
    // running agent is overridden by agent_cfg with the local endpoint above.
    if agent_state.control_url != public_url {
        agent_state.control_url = public_url.clone();
        agent_state.save(&agent_cfg.state_path())?;
    }

    // O host é o gateway .1: reivindica o endereço reservado antes de subir o
    // agente. Se outro nó já ocupou (ex.: control restaurado), mantém o IP atual.
    let gateway_ip = cfg.server_ip();
    if agent_state.overlay_ip != gateway_ip
        && !db::overlay_ip_in_use(&pool, &gateway_ip.to_string()).await?
    {
        db::set_node_overlay_ip(&pool, agent_state.node_id, &gateway_ip.to_string()).await?;
        agent_state.overlay_ip = gateway_ip;
        agent_state.save(&agent_cfg.state_path())?;
        println!("  gateway: {gateway_ip}");
    }

    let invite = mint_join_token(&pool, &actor, "convite", 20, 7 * 24 * 3600, &[]).await?;
    let quick = mint_join_token(&pool, &actor, "convite-rapido", 1, 3600, &[]).await?;
    let code = encode_connect_code(&public_url, &quick)
        .map_err(|err| anyhow::anyhow!(err))?;
    println!();
    println!("  malha pronta: {public_url}");
    println!("  painel:       {public_url}");
    println!();
    println!("  convite de um comando (1 uso):");
    println!("    curl -fsSL {public_url}/j/{quick} | sh");
    println!();
    println!("  ou entre com o código:");
    println!("    {code}");
    println!();
    println!("  convite manual (20 usos, 7 dias):");
    println!("    wm join {public_url} --token {invite}");
    println!();
    println!("  GUI local deste nó: wm gui");
    println!();

    if args.no_agent {
        let _ = server.await;
        return Ok(());
    }
    let result = wm_core::run(agent_cfg).await;
    server.abort();
    result
}

async fn run_serve(args: ServeArgs, root: PathBuf) -> anyhow::Result<()> {
    let mut cfg = match args.config {
        Some(path) => ControlConfig::load(&path)?,
        None => ControlConfig {
            data_dir: root,
            ..Default::default()
        },
    };
    cfg.apply_env();
    if let Some(listen) = args.listen {
        cfg.listen = listen;
    }
    if let Some(url) = args.public_url {
        cfg.public_url = url;
    }
    if args.tls_cert.is_some() {
        cfg.tls_cert = args.tls_cert;
    }
    if args.tls_key.is_some() {
        cfg.tls_key = args.tls_key;
    }
    args.stun.apply(&mut cfg);
    cfg.validate()?;
    wm_control::server::run(cfg).await
}

async fn run_join(args: JoinArgs, root: PathBuf) -> anyhow::Result<()> {
    let _instance = service::AgentLock::for_operation(&root, !args.no_start)?;
    let (url, join_token) = resolve_join_target(
        args.url.as_deref(),
        args.code.as_deref(),
        args.token.as_deref(),
    )?;
    let url = wm_core::control::parse_control_url(&url)?;
    let cfg = AgentConfig {
        data_dir: root.clone(),
        control_url: Some(url.clone()),
        udp_port: args.udp_port,
        mtu: args.mtu,
        tun_name: args.tun_name,
        name: args.name.clone(),
        tags: args.tags.clone(),
        labels: parse_labels(&args.labels)?,
        force_relay: args.force_relay,
        expose: parse_expose(&args.expose)?,
    };
    if let Some(existing) = wm_core::NodeState::load(&cfg.state_path())? {
        if !args.force {
            if args.no_start {
                println!("nó já inscrito ({} / {}); agente não iniciado", existing.node_name, existing.overlay_ip);
                return Ok(());
            }
            println!(
                "nó já inscrito ({} / {}); iniciando o agente",
                existing.node_name, existing.overlay_ip
            );
            return wm_core::run(cfg).await;
        }
    }
    let state = wm_core::join(&cfg, &url, &join_token, args.name.as_deref()).await?;
    println!("nó inscrito:");
    println!("  nome:    {}", state.node_name);
    println!("  overlay: {}", state.overlay_ip);
    println!("  control: {}", state.control_url);
    if args.no_start {
        println!("\ninicie com: wm up --data-dir {}", root.display());
        return Ok(());
    }
    wm_core::run(cfg).await
}

async fn run_up(args: AgentArgs, root: PathBuf) -> anyhow::Result<()> {
    let _instance = service::AgentLock::runtime(&root)?;
    let mut cfg = AgentConfig::with_data_dir(root);
    cfg.udp_port = args.udp_port;
    cfg.mtu = args.mtu;
    cfg.tun_name = args.tun_name;
    cfg.control_url = args.control;
    cfg.force_relay = args.force_relay;
    if !args.expose.is_empty() {
        cfg.expose = parse_expose(&args.expose)?;
    }
    wm_core::run(cfg).await
}

fn resolve_join_target(
    url: Option<&str>,
    code: Option<&str>,
    token: Option<&str>,
) -> anyhow::Result<(String, String)> {
    if let Some(code) = code.map(str::trim).filter(|code| !code.is_empty()) {
        return decode_connect_code(code).map_err(|err| anyhow::anyhow!(err));
    }
    let Some(raw) = url.map(str::trim).filter(|raw| !raw.is_empty()) else {
        bail!("informe a URL e --token, ou um --code wmc_...");
    };
    if raw.starts_with(CONNECT_CODE_PREFIX) {
        return decode_connect_code(raw).map_err(|err| anyhow::anyhow!(err));
    }
    if raw.starts_with("wm://join") {
        let (url, token) = decode_join_target(raw).map_err(|err| anyhow::anyhow!(err))?;
        let token = token.ok_or_else(|| anyhow::anyhow!("o link não contém um token"))?;
        return Ok((url, token));
    }
    let token = token
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| anyhow::anyhow!("informe --token ou use um código wmc_..."))?;
    Ok((raw.to_string(), token.to_string()))
}

async fn run_share(root: &Path, args: ShareArgs) -> anyhow::Result<()> {
    let kind = ShareKind::parse(&args.kind)
        .ok_or_else(|| anyhow::anyhow!("tipo inválido '{}': use web, game, tcp ou udp", args.kind))?;
    let name = args
        .name
        .clone()
        .unwrap_or_else(|| format!("port-{}", args.port));
    let protocol = match args.protocol.as_deref() {
        Some(raw) => Some(
            ShareProtocol::parse(raw)
                .ok_or_else(|| anyhow::anyhow!("protocolo inválido '{raw}': use tcp, udp ou both"))?,
        ),
        None => None,
    };
    let mut params = json!({
        "name": name,
        "port": args.port,
        "kind": kind,
        "description": args.description.trim(),
        "enabled": !args.disabled,
    });
    if let Some(protocol) = protocol {
        params["protocol"] = json!(protocol);
    }
    let value = local_call(root, "share_create", params).await?;
    let share: ShareInfo = serde_json::from_value(
        value
            .get("share")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("resposta sem share"))?,
    )?;
    println!("share publicado: {}", share.wm_url());
    println!("  nome:     {}", share.name);
    println!("  endereço: {}", share.address());
    if let Some(url) = share.open_url() {
        println!("  url:      {url}");
    }
    if share.protocol.includes_udp() {
        let transport = if share.protocol.includes_tcp() {
            "tcp+udp"
        } else {
            "udp"
        };
        println!("  transporte: {transport}");
    }
    println!("
para ver: wm shares   ·   para abrir: wm open {}", share.name);
    Ok(())
}

async fn get_shares(root: &Path) -> anyhow::Result<Vec<ShareInfo>> {
    let value = local_call(root, "shares", Value::Null).await?;
    serde_json::from_value(value).context("resposta de shares inválida")
}

async fn run_shares(root: &Path, as_json: bool) -> anyhow::Result<()> {
    let shares = get_shares(root).await?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&shares)?);
        return Ok(());
    }
    if shares.is_empty() {
        println!("nenhum share publicado na malha");
        println!("publique um com: wm share 3000 --name demo --kind web");
        return Ok(());
    }
    println!(
        "{:<20} {:<14} {:<7} {:<9} {:<24} {}",
        "NOME", "DONO", "TIPO", "TRANSP.", "ENDEREÇO", "ESTADO"
    );
    for share in shares {
        println!(
            "{:<20} {:<14} {:<7} {:<9} {:<24} {}",
            share.name,
            share.node_name,
            share.kind.as_str(),
            share.protocol.as_str(),
            share.address(),
            if share.enabled { "ativo" } else { "desligado" }
        );
    }
    println!("
abra um web com `wm open <nome>`; jogos usam o endereço IP:porta.");
    Ok(())
}

async fn find_share(root: &Path, name: &str) -> anyhow::Result<ShareInfo> {
    let name = name
        .trim()
        .strip_prefix("wm://")
        .unwrap_or(name.trim())
        .trim_end_matches('/');
    get_shares(root)
        .await?
        .into_iter()
        .find(|share| share.name == name || share.id.to_string() == name)
        .ok_or_else(|| anyhow::anyhow!("share '{name}' não encontrado"))
}

async fn run_unshare(root: &Path, name: &str) -> anyhow::Result<()> {
    let share = find_share(root, name).await?;
    local_call(root, "share_delete", json!({ "id": share.id })).await?;
    println!("share removido: {}", share.name);
    Ok(())
}

async fn run_share_rename(root: &Path, name: &str, new_name: &str) -> anyhow::Result<()> {
    let share = find_share(root, name).await?;
    local_call(
        root,
        "share_update",
        json!({ "id": share.id, "name": new_name.trim() }),
    )
    .await?;
    println!("share renomeado: {} -> {}", share.name, new_name.trim());
    Ok(())
}

async fn run_share_toggle(root: &Path, name: &str, enabled: bool) -> anyhow::Result<()> {
    let share = find_share(root, name).await?;
    local_call(
        root,
        "share_update",
        json!({ "id": share.id, "enabled": enabled }),
    )
    .await?;
    println!(
        "share {}: {}",
        share.name,
        if enabled { "ligado" } else { "desligado" }
    );
    Ok(())
}

async fn run_rename(root: &Path, name: &str) -> anyhow::Result<()> {
    let value = local_call(root, "rename_node", json!({ "name": name.trim() })).await?;
    let new_name = value
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(name.trim());
    println!("nó renomeado para {new_name}");
    Ok(())
}

async fn run_open(root: &Path, target: &str, print_only: bool) -> anyhow::Result<()> {
    let value = local_call(root, "share_resolve", json!({ "name": target })).await?;
    let share: ShareInfo = serde_json::from_value(
        value
            .get("share")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("resposta sem share"))?,
    )?;
    if let Some(url) = value.get("url").and_then(Value::as_str) {
        println!("{}: {url}", share.name);
        if !print_only {
            open_url(url);
        }
        return Ok(());
    }
    println!("{} ({}) — {}", share.name, share.kind.as_str(), share.address());
    if share.kind == ShareKind::Game {
        println!("cole o endereço no jogo. Steam: steam://connect/{}", share.address());
    } else {
        println!("copie o endereço e use no cliente do serviço.");
    }
    Ok(())
}

fn open_url(url: &str) {
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(target_os = "linux")]
    let program = "xdg-open";
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let program = "";
    if program.is_empty() {
        return;
    }
    let _ = std::process::Command::new(program)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

async fn run_down(root: &Path) -> anyhow::Result<()> {
    let pid_path = root.join("agent.pid");
    let text = std::fs::read_to_string(&pid_path)
        .with_context(|| format!("agente não está rodando ({})", pid_path.display()))?;
    let pid: i32 = text.trim().parse().context("pid inválido")?;
    if pid <= 0 {
        bail!("pid inválido em {}", pid_path.display());
    }
    let status = std::process::Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()?;
    if !status.success() {
        bail!("kill falhou para pid {pid}");
    }
    // SIGTERM termina o processo sem o cleanup do agente; espere o processo
    // sumir e remova os arquivos que ele deixaria para trás.
    for _ in 0..50 {
        if unsafe { libc::kill(pid, 0) } != 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let _ = std::fs::remove_file(&pid_path);
    let _ = std::fs::remove_file(root.join("agent.sock"));
    println!("agente parado");
    Ok(())
}

async fn run_status(root: &Path, as_json: bool) -> anyhow::Result<()> {
    match local_call(root, "status", Value::Null).await {
        Ok(value) => {
            if as_json {
                println!("{}", serde_json::to_string_pretty(&value)?);
            } else {
                print_status(&value);
            }
        }
        Err(err) => {
            let state = wm_core::NodeState::load(&root.join("agent-state.json"))?;
            if as_json {
                let value = match state {
                    Some(s) => json!({
                        "running": false,
                        "node_id": s.node_id,
                        "node_name": s.node_name,
                        "overlay_ip": s.overlay_ip,
                        "control_url": s.control_url,
                        "error": err.to_string(),
                    }),
                    None => json!({ "running": false, "error": err.to_string() }),
                };
                println!("{}", serde_json::to_string_pretty(&value)?);
            } else {
                println!("agente parado ({err})");
                if let Some(s) = state {
                    println!("nó:     {}", s.node_name);
                    println!("overlay: {}", s.overlay_ip);
                    println!("control: {}", s.control_url);
                    println!("inicie com: wm up --data-dir {}", root.display());
                }
            }
        }
    }
    Ok(())
}

fn print_status(value: &Value) {
    let name = value.get("node_name").and_then(Value::as_str).unwrap_or("?");
    let ip = value.get("overlay_ip").and_then(Value::as_str).unwrap_or("?");
    let tun = value.get("tun_name").and_then(Value::as_str).unwrap_or("?");
    let control = value
        .get("control_connected")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let relay = value
        .get("relay_connected")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let peers = value.get("peers_total").and_then(Value::as_u64).unwrap_or(0);
    let online = value.get("peers_online").and_then(Value::as_u64).unwrap_or(0);
    let map = value.get("map_version").and_then(Value::as_u64).unwrap_or(0);
    println!("{name}  {ip}  tun={tun}");
    println!(
        "control: {}   relay: {}   mapa: v{map}   peers: {online}/{peers}",
        if control { "ok" } else { "off" },
        if relay { "ok" } else { "off" }
    );
    if let Some(err) = value.get("last_error").and_then(Value::as_str) {
        println!("último erro: {err}");
    }
}

async fn run_peers(root: &Path, as_json: bool) -> anyhow::Result<()> {
    match local_call(root, "peers", Value::Null).await {
        Ok(value) => {
            if as_json {
                println!("{}", serde_json::to_string_pretty(&value)?);
            } else if let Some(peers) = value.as_array() {
                if peers.is_empty() {
                    println!("nenhum peer no mapa ainda");
                }
                for peer in peers {
                    println!(
                        "{:<20} {:<13} {:<8} {:<21} rtt={:<6} hs={:<5} rx={} tx={}",
                        peer.get("name").and_then(Value::as_str).unwrap_or("?"),
                        peer.get("overlay_ip").and_then(Value::as_str).unwrap_or("?"),
                        peer.get("path").and_then(Value::as_str).unwrap_or("?"),
                        peer.get("endpoint")
                            .and_then(Value::as_str)
                            .unwrap_or("—"),
                        peer.get("rtt_ms")
                            .and_then(Value::as_u64)
                            .map(|v| format!("{v}ms"))
                            .unwrap_or_else(|| "—".into()),
                        peer.get("last_handshake_secs")
                            .and_then(Value::as_u64)
                            .map(|v| format!("{v}s"))
                            .unwrap_or_else(|| "—".into()),
                        peer.get("rx_bytes").and_then(Value::as_u64).unwrap_or(0),
                        peer.get("tx_bytes").and_then(Value::as_u64).unwrap_or(0),
                    );
                }
            }
        }
        Err(err) => {
            if as_json {
                println!("{}", json!({ "error": err.to_string() }));
            } else {
                println!("agente parado ({err})");
            }
        }
    }
    Ok(())
}

async fn run_local(root: &Path, method: &str, params: Value) -> anyhow::Result<()> {
    let value = local_call(root, method, params).await?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

async fn run_extensions(
    root: &Path,
    as_json: bool,
    action: Option<ExtensionsAction>,
) -> anyhow::Result<()> {
    match action {
        Some(ExtensionsAction::Enable { id }) => {
            let value = local_call(
                root,
                "extension_activation",
                json!({ "id": id, "active": true }),
            )
            .await?;
            if as_json {
                println!("{}", serde_json::to_string_pretty(&value)?);
            } else {
                println!("extensão '{id}' ativada neste nó");
            }
        }
        Some(ExtensionsAction::Disable { id }) => {
            let value = local_call(
                root,
                "extension_activation",
                json!({ "id": id, "active": false }),
            )
            .await?;
            if as_json {
                println!("{}", serde_json::to_string_pretty(&value)?);
            } else {
                println!("extensão '{id}' desativada neste nó");
            }
        }
        None => {
            let value = local_call(root, "extension_catalog", Value::Null).await?;
            let catalog: Vec<wm_proto::NodeExtensionView> = serde_json::from_value(value)?;
            if as_json {
                println!("{}", serde_json::to_string_pretty(&catalog)?);
            } else if catalog.is_empty() {
                println!("nenhuma extensão no catálogo");
            } else {
                println!("{:<20} {:<12} {:<6} {:<10} mensagem", "EXTENSÃO", "DRIVER", "ATIVA", "ESTADO");
                for view in catalog {
                    let (state, message) = view
                        .status
                        .as_ref()
                        .map(|status| (status.state.to_string(), status.message.clone().unwrap_or_default()))
                        .unwrap_or_else(|| ("—".into(), String::new()));
                    println!(
                        "{:<20} {:<12} {:<6} {:<10} {}",
                        view.id,
                        view.driver,
                        if view.active { "on" } else { "off" },
                        state,
                        message
                    );
                }
            }
        }
    }
    Ok(())
}

async fn run_tui(root: &Path, args: TuiArgs) -> anyhow::Result<()> {
    wm_tui::run_with(
        root.to_path_buf(),
        wm_tui::RunOptions {
            no_auto_start: args.no_auto_start,
            no_mouse: args.no_mouse,
        },
    )
}

async fn run_doctor(root: &Path) -> anyhow::Result<()> {
    let mut ok = true;
    println!("wired-magic doctor");
    println!("------------------");
    let state = wm_core::NodeState::load(&root.join("agent-state.json"))?;
    match &state {
        Some(_) => println!("[ok] estado do nó encontrado em {}", root.display()),
        None => {
            println!("[--] sem estado em {} (rode wm join)", root.display());
            ok = false;
        }
    }
    let is_root = unsafe { libc::geteuid() } == 0;
    println!(
        "[{}] privilégio: {}",
        if is_root { "ok" } else { "--" },
        if is_root {
            "root"
        } else {
            "sem root; TUN/rotas vão falhar"
        }
    );
    ok &= is_root;

    if let Some(state) = &state {
        let control = wm_core::control::ControlClient::new(&state.control_url)?;
        match control.meta().await {
            Ok(meta) => println!(
                "[ok] control {} (proto {}) relay={:?}",
                state.control_url, meta.protocol, meta.relay
            ),
            Err(err) => {
                println!("[--] control {}: {err}", state.control_url);
                ok = false;
            }
        }
    }
    match local_call(root, "status", Value::Null).await {
        Ok(_) => println!("[ok] agente respondendo no socket local"),
        Err(err) => {
            println!("[--] agente local: {err}");
            ok = false;
        }
    }
    #[cfg(target_os = "macos")]
    let route = std::process::Command::new("route")
        .args(["-n", "get", "10.66.0.1"])
        .output();
    #[cfg(not(target_os = "macos"))]
    let route = std::process::Command::new("ip")
        .args(["route", "get", "10.66.0.1"])
        .output();
    match route {
        Ok(out) if out.status.success() => println!("[ok] rota para 10.66.0.1 presente"),
        _ => println!("[--] rota para 10.66.0.1 ausente (agente parado?)"),
    }
    println!();
    if ok {
        println!("tudo certo.");
    } else {
        println!("há itens pendentes acima.");
    }
    Ok(())
}

async fn run_service(args: ServiceArgs, root: &Path) -> anyhow::Result<()> {
    service::run(args.action, root)
}

fn run_cmd(bin: &str, args: &[&str]) -> anyhow::Result<()> {
    let out = std::process::Command::new(bin).args(args).output()?;
    if !out.status.success() {
        bail!(
            "{} {}: {}",
            bin,
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

async fn local_call(root: &Path, method: &str, params: Value) -> anyhow::Result<Value> {
    let socket = root.join("agent.sock");
    let stream = UnixStream::connect(&socket)
        .await
        .with_context(|| format!("agente não está ouvindo em {}", socket.display()))?;
    let (read, mut write) = stream.into_split();
    let req = LocalRequest {
        id: 1,
        method: method.to_string(),
        params,
    };
    let mut line = serde_json::to_vec(&req)?;
    line.push(b'\n');
    write.write_all(&line).await?;
    write.flush().await?;
    let mut reader = BufReader::new(read);
    let mut resp = String::new();
    reader.read_line(&mut resp).await?;
    let parsed: LocalResponse =
        serde_json::from_str(&resp).context("resposta inválida do agente")?;
    if parsed.ok {
        Ok(parsed.result.unwrap_or(Value::Null))
    } else {
        bail!(parsed.error.unwrap_or_else(|| "erro no agente".into()))
    }
}

fn parse_expose(values: &[String]) -> anyhow::Result<Vec<wm_core::state::ExposeSpec>> {
    let mut out = Vec::new();
    for value in values {
        let (overlay, local) = match value.split_once(':') {
            Some((a, b)) => (a, b),
            None => (value.as_str(), value.as_str()),
        };
        let overlay_port: u16 = overlay
            .parse()
            .with_context(|| format!("porta overlay inválida em '{value}'"))?;
        let local_port: u16 = local
            .parse()
            .with_context(|| format!("porta local inválida em '{value}'"))?;
        out.push(wm_core::state::ExposeSpec {
            overlay_port,
            local_port,
        });
    }
    Ok(out)
}

fn parse_labels(pairs: &[String]) -> anyhow::Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for pair in pairs {
        let (k, v) = pair
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("label precisa ser chave=valor: {pair}"))?;
        if k.trim().is_empty() {
            bail!("label com chave vazia");
        }
        out.insert(k.trim().to_string(), v.trim().to_string());
    }
    Ok(out)
}

async fn mint_join_token(
    pool: &sqlx::SqlitePool,
    actor: &str,
    name: &str,
    uses: i64,
    ttl_secs: i64,
    tags: &[String],
) -> anyhow::Result<String> {
    let token = auth::random_token("wmj_")?;
    let prefix: String = token.chars().take(12).collect();
    db::create_join_token(
        pool,
        name,
        &prefix,
        &auth::hash_token(&token),
        tags,
        &BTreeMap::new(),
        uses,
        ttl_secs,
        actor,
    )
    .await?;
    Ok(token)
}

// With native TLS, retain the public hostname for certificate validation.
// Behind a TLS-terminating proxy, bootstrap directly over the local HTTP listener.
fn host_control_url(addr: SocketAddr, native_tls: bool, public_url: &str) -> String {
    if native_tls {
        return public_url.to_string();
    }
    let ip = match addr.ip() {
        std::net::IpAddr::V4(ip) if ip.is_unspecified() => std::net::Ipv4Addr::LOCALHOST.into(),
        std::net::IpAddr::V6(ip) if ip.is_unspecified() => std::net::Ipv6Addr::LOCALHOST.into(),
        ip => ip,
    };
    format!("http://{}", SocketAddr::new(ip, addr.port()))
}

async fn wait_for_port(addr: SocketAddr) -> anyhow::Result<()> {
    let ip = if addr.ip().is_unspecified() {
        "127.0.0.1".parse().unwrap()
    } else {
        addr.ip()
    };
    for _ in 0..100 {
        if TcpStream::connect((ip, addr.port())).await.is_ok() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    bail!("control plane não abriu em {addr}")
}

#[cfg(test)]
mod port_tests {
    use super::*;

    #[test]
    fn host_bootstrap_bypasses_public_proxy() {
        let public = "https://mesh.example";
        for (listen, expected) in [
            ("0.0.0.0:8080", "http://127.0.0.1:8080"),
            ("[::]:8081", "http://[::1]:8081"),
            ("127.0.0.1:9000", "http://127.0.0.1:9000"),
        ] {
            assert_eq!(host_control_url(listen.parse().unwrap(), false, public), expected);
            assert_eq!(host_control_url(listen.parse().unwrap(), true, public), public);
        }
    }

    #[test]
    fn host_accepts_custom_ports_and_derives_stun_advertisement() {
        let cli = Cli::try_parse_from([
            "wm", "host", "--listen", "0.0.0.0:8443", "--public-url",
            "https://mesh.example:8443", "--stun-listen", "0.0.0.0:5349",
            "--udp-port", "51820",
        ]).unwrap();
        let Command::Host(args) = cli.command else { panic!("expected host") };
        assert_eq!(args.listen.port(), 8443);
        assert_eq!(args.udp_port, 51820);
        let mut cfg = ControlConfig {
            public_url: args.public_url.unwrap(),
            ..Default::default()
        };
        args.stun.apply(&mut cfg);
        cfg.validate().unwrap();
        assert_eq!(cfg.stun_urls, ["stun:mesh.example:5349"]);
        assert_eq!(cfg.relay_public_url(), "https://mesh.example:8443/relay");
    }

    #[test]
    fn serve_accepts_separate_public_stun_ports() {
        let cli = Cli::try_parse_from([
            "wm", "serve", "--stun-listen", "0.0.0.0:5349",
            "--stun-url", "stun:mesh.example:443",
            "--stun-url", "stun:backup.example:5350",
        ]).unwrap();
        let Command::Serve(args) = cli.command else { panic!("expected serve") };
        let mut cfg = ControlConfig::default();
        args.stun.apply(&mut cfg);
        cfg.validate().unwrap();
        assert_eq!(cfg.stun_listen.port(), 5349);
        assert_eq!(cfg.stun_urls, ["stun:mesh.example:443", "stun:backup.example:5350"]);
    }

    #[test]
    fn rejects_invalid_stun_bind_address() {
        assert!(Cli::try_parse_from(["wm", "host", "--stun-listen", "0.0.0.0:99999"]).is_err());
    }
}

