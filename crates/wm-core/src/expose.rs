//! Allowlisted loopback forwarding, reconciled from the map.
//!
//! Many useful services (the Seance agent's Baguette, Vite, a local API) bind
//! only to 127.0.0.1. Binding the same port on the overlay address and proxying
//! to loopback exposes exactly that service to the mesh, subject to the ACL
//! applied by the data plane before packets reach the kernel.
//!
//! The local `--expose`/state specs are the base; active extension adapters add
//! more through `NetworkMap.exposes`. The listener set is reconciled on every
//! change, so activating an adapter does not require restarting the agent.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::time::Duration;

use tokio::io::copy_bidirectional;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::state::ExposeSpec;

struct Listener {
    local_port: u16,
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.task.abort();
    }
}

pub async fn run(
    overlay_ip: Ipv4Addr,
    mut desired: watch::Receiver<Vec<ExposeSpec>>,
) -> anyhow::Result<()> {
    let mut active: BTreeMap<u16, Listener> = BTreeMap::new();
    let initial = desired.borrow_and_update().clone();
    reconcile(overlay_ip, &mut active, initial);
    while desired.changed().await.is_ok() {
        let next = desired.borrow_and_update().clone();
        reconcile(overlay_ip, &mut active, next);
    }
    active.clear();
    Ok(())
}

fn reconcile(overlay_ip: Ipv4Addr, active: &mut BTreeMap<u16, Listener>, specs: Vec<ExposeSpec>) {
    let mut wanted: BTreeMap<u16, u16> = BTreeMap::new();
    for spec in specs {
        if spec.overlay_port == 0 || spec.local_port == 0 {
            continue;
        }
        if wanted.insert(spec.overlay_port, spec.local_port).is_some() {
            tracing::warn!(
                port = spec.overlay_port,
                "expose: porta de overlay duplicada; mantendo a primeira"
            );
        }
    }

    let remove: Vec<u16> = active
        .keys()
        .filter(|port| !wanted.contains_key(port))
        .copied()
        .collect();
    for port in remove {
        if let Some(listener) = active.remove(&port) {
            tracing::info!(port, "expose: removido");
            drop(listener);
        }
    }

    for (overlay_port, local_port) in wanted {
        let restart = active
            .get(&overlay_port)
            .map(|listener| listener.local_port != local_port)
            .unwrap_or(false);
        if restart {
            if let Some(listener) = active.remove(&overlay_port) {
                drop(listener);
            }
        }
        if active.contains_key(&overlay_port) {
            continue;
        }
        let cancel = CancellationToken::new();
        let task = tokio::spawn(listen(
            overlay_ip,
            overlay_port,
            local_port,
            cancel.clone(),
        ));
        active.insert(
            overlay_port,
            Listener {
                local_port,
                cancel,
                task,
            },
        );
        tracing::info!(port = overlay_port, local = local_port, "expose: desejado");
    }
}

async fn listen(
    overlay_ip: Ipv4Addr,
    overlay_port: u16,
    local_port: u16,
    cancel: CancellationToken,
) {
    let listener = loop {
        match TcpListener::bind((overlay_ip, overlay_port)).await {
            Ok(listener) => break listener,
            Err(err) => {
                tracing::warn!(
                    %err,
                    port = overlay_port,
                    "expose: bind falhou; tentando de novo em 5s"
                );
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                }
            }
        }
    };
    tracing::info!(
        overlay = %format!("{overlay_ip}:{overlay_port}"),
        local = %format!("127.0.0.1:{local_port}"),
        "serviço exposto no overlay"
    );
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            accepted = listener.accept() => {
                let Ok((incoming, peer)) = accepted else {
                    continue;
                };
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
        }
    }
}
