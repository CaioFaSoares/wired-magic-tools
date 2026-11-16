//! Explicit-server administrative CLI. Never uses a node enrollment token.
use std::{io::{self, Read}, path::PathBuf};
use anyhow::{Context, bail};
use clap::{Args, Subcommand};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;
use wm_ext::admin::{AdminClient, AdminClientOptions, AdminSession};

#[derive(Args)]
pub struct AdminArgs {
    /// URL do servidor a administrar. Nunca inferida de outro login.
    #[arg(long)]
    pub server: String,
    /// Arquivo de sessão confidencial; login cria um arquivo novo (0600).
    #[arg(long)]
    pub session_file: Option<PathBuf>,
    /// Permite HTTP sem criptografia na malha; prefira HTTPS.
    #[arg(long)]
    pub allow_insecure_http_overlay: bool,
    #[command(subcommand)]
    pub action: AdminAction,
}

#[derive(Subcommand)]
pub enum AdminAction {
    /// Abre painel administrativo; sessão apenas em memória.
    Tui,
    /// Lê senha de stdin. Não coloque senha em argumentos/histórico.
    Login { #[arg(long, default_value = "admin")] username: String, #[arg(long)] password_stdin: bool },
    Logout,
    Overview,
    Nodes,
    Approve { node: Uuid },
    Revoke { node: Uuid, #[arg(long)] yes: bool },
    /// Mostra ACL; --file envia política YAML após validação no servidor.
    Acl { #[arg(long)] file: Option<PathBuf> },
    JoinTokens,
    /// Cria convite de inscrição com aprovação obrigatória por padrão.
    CreateJoinToken { name: String, #[arg(long, default_value_t = 86400)] ttl_secs: i64, #[arg(long, default_value_t = 1)] max_uses: i64, #[arg(long)] auto_approve: bool },
    RevokeJoinToken { token: Uuid, #[arg(long)] yes: bool },
    Extensions,
    ReloadExtensions,
    Extension { id: String },
    /// Configuração do componente de uma extensão em um nó específico.
    Configure { id: String, node: Uuid, #[arg(long)] file: Option<PathBuf>, #[arg(long, requires = "file")] disabled: bool },
    Activate { id: String, node: Uuid, #[arg(long)] off: bool },
}

#[cfg(test)]
mod tests {
    use crate::Cli;
    use clap::Parser;

    #[test]
    fn admin_cli_requires_server_and_never_accepts_password_argument() {
        assert!(Cli::try_parse_from(["wm", "admin", "nodes"]).is_err());
        assert!(Cli::try_parse_from(["wm", "admin", "--server", "https://one.example", "login", "--password", "secret"]).is_err());
        assert!(Cli::try_parse_from(["wm", "admin", "--server", "https://one.example", "tui"]).is_ok());
    }

    #[test]
    fn disabled_config_requires_a_payload() {
        let node = "00000000-0000-0000-0000-000000000001";
        assert!(Cli::try_parse_from(["wm", "admin", "--server", "https://one.example", "configure", "chat_gui", node, "--disabled"]).is_err());
        assert!(Cli::try_parse_from(["wm", "admin", "--server", "https://one.example", "configure", "chat_gui", node, "--disabled", "--file", "config.json"]).is_ok());
    }
}

fn print(value: &impl Serialize) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

pub async fn run(args: AdminArgs) -> anyhow::Result<()> {
    if matches!(args.action, AdminAction::Tui) {
        return tokio::task::spawn_blocking(move || wm_tui::remote_admin::run(Some(args.server))).await?;
    }
    let client = AdminClient::with_options(&args.server, AdminClientOptions {
        allow_insecure_http_overlay: args.allow_insecure_http_overlay,
        ..Default::default()
    })?;
    let path = args.session_file.as_ref().context("informe --session-file para guardar/carregar a sessão deste servidor")?;
    if let AdminAction::Login { username, password_stdin } = args.action {
        if !password_stdin { bail!("use login --password-stdin; senha não é aceita em argumentos"); }
        let mut password = String::new();
        io::stdin().take(4097).read_to_string(&mut password)?;
        if password.len() > 4096 { bail!("senha excede limite"); }
        let password = password.trim_end_matches(['\r', '\n']);
        if password.is_empty() { bail!("senha vazia"); }
        let session = client.login(&username, password).await?;
        if let Err(error) = session.save(path) {
            // Do not leave a usable orphan session after failed persistence.
            let _ = client.logout(&session).await;
            return Err(error).context("não foi possível criar arquivo de sessão; use um caminho novo");
        }
        println!("sessão administrativa criada para {}", client.server());
        return Ok(());
    }
    let session = AdminSession::load(path, client.server())?;
    match args.action {
        AdminAction::Login { .. } | AdminAction::Tui => unreachable!(),
        AdminAction::Logout => print(&client.logout(&session).await?),
        AdminAction::Overview => print(&client.overview(&session).await?),
        AdminAction::Nodes => print(&client.nodes(&session).await?),
        AdminAction::Approve { node } => print(&client.approve_node(&session, node).await?),
        AdminAction::Revoke { node, yes } => {
            if !yes { bail!("revogação exige --yes"); }
            print(&client.revoke_node(&session, node).await?)
        }
        AdminAction::Acl { file } => {
            if let Some(file) = file {
                let policy = std::fs::read_to_string(file)?;
                client.validate_acl(&session, &policy).await?;
                print(&client.put_acl(&session, &policy).await?)
            } else { print(&client.acl(&session).await?) }
        }
        AdminAction::JoinTokens => print(&client.join_tokens(&session).await?),
        AdminAction::CreateJoinToken { name, ttl_secs, max_uses, auto_approve } => {
            if ttl_secs <= 0 || max_uses <= 0 { bail!("TTL e usos devem ser positivos"); }
            print(&client.create_join_token(&session, &wm_proto::CreateJoinTokenRequest {
                name, ttl_secs, max_uses, tags: Vec::new(), labels: Default::default(), require_approval: !auto_approve,
            }).await?)
        }
        AdminAction::RevokeJoinToken { token, yes } => {
            if !yes { bail!("revogação exige --yes"); }
            print(&client.revoke_join_token(&session, token).await?)
        }
        AdminAction::Extensions => print(&client.extensions(&session).await?),
        AdminAction::ReloadExtensions => print(&client.reload_extensions(&session).await?),
        AdminAction::Extension { id } => print(&client.extension(&session, &id).await?),
        AdminAction::Configure { id, node, file, disabled } => {
            if let Some(file) = file {
                let config: Value = serde_json::from_slice(&std::fs::read(file)?)?;
                print(&client.put_extension_config(&session, &id, node, !disabled, &config).await?)
            } else { print(&client.extension_config(&session, &id, node).await?) }
        }
        AdminAction::Activate { id, node, off } => print(&client.set_extension_activation(&session, &id, node, !off).await?),
    }
}
