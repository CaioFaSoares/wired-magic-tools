//! Allowlisted loopback forwarding.
//!
//! Many useful services (the Seance agent's Baguette, Vite, a local API) bind
//! only to 127.0.0.1. Binding the same port on the overlay address and proxying
//! to loopback exposes exactly that service to the mesh, subject to the ACL
//! applied by the data plane before packets reach the kernel.

use std::net::Ipv4Addr;

use anyhow::Context;
use tokio::io::copy_bidirectional;
use tokio::net::{TcpListener, TcpStream};

use crate::state::ExposeSpec;

pub async fn run(overlay_ip: Ipv4Addr, specs: Vec<ExposeSpec>) -> anyhow::Result<()> {
    let mut seen = std::collections::HashSet::new();
    let mut handles = Vec::new();
    for spec in specs {
        if spec.overlay_port == 0 || spec.local_port == 0 || !seen.insert(spec.overlay_port) {
            continue;
        }
        let listener = TcpListener::bind((overlay_ip, spec.overlay_port))
            .await
            .with_context(|| format!("expor {overlay_ip}:{}", spec.overlay_port))?;
        tracing::info!(
            overlay = %format!("{overlay_ip}:{}", spec.overlay_port),
            local = %format!("127.0.0.1:{}", spec.local_port),
            "serviço exposto no overlay"
        );
        handles.push(tokio::spawn(async move {
            loop {
                let Ok((incoming, peer)) = listener.accept().await else {
                    continue;
                };
                let local_port = spec.local_port;
                tokio::spawn(async move {
                    match TcpStream::connect(("127.0.0.1", local_port)).await {
                        Ok(mut local) => {
                            let mut incoming = incoming;
                            if let Err(err) = copy_bidirectional(&mut incoming, &mut local).await {
                                tracing::debug!(%err, %peer, "forward encerrado");
                            }
                        }
                        Err(err) => {
                            tracing::warn!(%err, %peer, local_port, "serviço local indisponível");
                        }
                    }
                });
            }
        }));
    }
    if handles.is_empty() {
        return Ok(());
    }
    // Keep the task alive for the lifetime of the agent; the handles are
    // aborted when the process exits.
    for handle in handles {
        let _ = handle.await;
    }
    Ok(())
}
