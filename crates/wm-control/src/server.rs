//! Long-running control-plane server.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use axum_server::Handle;
use axum_server::tls_rustls::RustlsConfig;

use wm_proto::ControlEvent;

use crate::api;
use crate::config::ControlConfig;
use crate::db;
use crate::state::AppState;
use crate::stun;

pub async fn run(cfg: ControlConfig) -> anyhow::Result<()> {
    let bind = cfg.listen;
    let use_tls = cfg.tls_cert.is_some();
    let public_url = cfg.public_url.clone();
    let (state, generated_password) = AppState::new(cfg).await?;

    if let Some(password) = &generated_password {
        println!();
        println!("  wired-magic — primeiro start");
        println!("  painel:   {public_url}");
        println!("  usuário:  admin");
        println!("  senha:    {password}");
        println!("  (guardada só como hash; anote agora)");
        println!();
    }

    if state.cfg.stun_enabled {
        let listen = state.cfg.stun_listen;
        tokio::spawn(async move {
            if let Err(err) = stun::run_stun_server(listen).await {
                tracing::error!(%err, "STUN parou");
            }
        });
    }

    let presence_state = state.clone();
    tokio::spawn(async move { presence_loop(presence_state).await });

    let app = api::router(state.clone());
    let handle = Handle::new();
    let shutdown_handle = handle.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("sinal recebido; encerrando");
        shutdown_handle.graceful_shutdown(Some(Duration::from_secs(5)));
    });

    tracing::info!(%bind, tls = use_tls, url = %state.cfg.public_url, "control plane ouvindo");
    if use_tls {
        let cert = state.cfg.tls_cert.as_ref().expect("checked");
        let key = state.cfg.tls_key.as_ref().expect("checked");
        let tls = RustlsConfig::from_pem_file(cert, key)
            .await
            .with_context(|| format!("carregando TLS {cert:?} / {key:?}"))?;
        axum_server::bind_rustls(bind, tls)
            .handle(handle)
            .serve(app.into_make_service_with_connect_info::<std::net::SocketAddr>())
            .await?;
    } else {
        axum_server::bind(bind)
            .handle(handle)
            .serve(app.into_make_service_with_connect_info::<std::net::SocketAddr>())
            .await?;
    }
    state.shutdown().await;
    Ok(())
}

async fn presence_loop(state: Arc<AppState>) {
    let mut tick = tokio::time::interval(Duration::from_secs(10));
    loop {
        tick.tick().await;
        let now = db::now_ts();
        let Ok(nodes) = db::list_nodes(&state.db).await else {
            continue;
        };
        for node in nodes {
            if node.status() != wm_proto::NodeStatus::Active {
                continue;
            }
            let online = node.online(now) || state.relay.is_online(node.uuid());
            if let Some(reported) = state.mark_presence(node.uuid(), online).await {
                state.broadcast(ControlEvent::Presence {
                    node_id: node.uuid(),
                    online: reported,
                });
            }
        }
    }
}
