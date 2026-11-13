use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;

use wm_proto::{NodeId, NodeStatus, RelayFrame};

use super::{ApiErr, client_ip};
use crate::auth;
use crate::db;
use crate::state::AppState;

pub async fn handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiErr> {
    let token = auth::bearer_token(&headers)
        .ok_or_else(|| ApiErr::unauthorized("relay_token_missing", "token de relay ausente"))?;
    let claims = auth::verify_relay_token(
        &state.control_pubkey,
        token,
        &state.relay_audience(),
    )
    .map_err(|e| ApiErr::unauthorized("relay_token_invalid", e.to_string()))?;
    let Some(node) = db::node_by_id(&state.db, claims.sub).await? else {
        return Err(ApiErr::unauthorized("node_unknown", "nó do token não existe"));
    };
    if node.status() != NodeStatus::Active {
        return Err(ApiErr::forbidden("node_inactive", "nó não está ativo"));
    }
    tracing::debug!(node = %node.name, ip = ?client_ip(&headers), "relay: upgrade");
    Ok(ws
        .max_message_size(wm_proto::RELAY_MAX_PAYLOAD + 64)
        .on_upgrade(move |socket| handle(state, claims.sub, socket)))
}

async fn handle(state: Arc<AppState>, node: NodeId, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(512);
    let conn_id = state.relay.register(node, tx.clone());
    let Some(mut cancelled) = state.relay.cancellation(node, conn_id) else { return; };
    // Recheck after registration: revocation may have raced the HTTP upgrade.
    if !node_is_active(&state.db, node).await {
        state.relay.unregister(node, conn_id);
        return;
    }

    // Tell the newcomer who is online, then tell everyone about the newcomer.
    for peer in state.relay.online_nodes() {
        if peer != node {
            if let Ok(frame) = (RelayFrame::Presence {
                node: peer,
                online: true,
            })
            .encode()
            {
                let _ = tx.try_send(frame);
            }
        }
    }
    state.relay.announce_online(node, conn_id);

    let writer = async move {
        let mut ping = tokio::time::interval(Duration::from_secs(25));
        loop {
            tokio::select! {
                _ = ping.tick() => {
                    if let Ok(frame) = RelayFrame::Ping.encode() {
                        if sink.send(Message::Binary(frame.into())).await.is_err() {
                            break;
                        }
                    }
                }
                msg = rx.recv() => {
                    match msg {
                        Some(bytes) => {
                            if sink.send(Message::Binary(bytes.into())).await.is_err() {
                                break;
                            }
                        }
                        None => break,
                    }
                }
            }
        }
    };
    tokio::pin!(writer);

    loop {
        let msg = tokio::select! {
            biased;
            _ = cancelled.changed() => break,
            _ = &mut writer => break,
            msg = stream.next() => match msg { Some(msg) => msg, None => break },
        };
        if *cancelled.borrow() { break; }
        let Ok(msg) = msg else { break };
        match msg {
            Message::Binary(bytes) => {
                match RelayFrame::decode(&bytes) {
                    Ok(RelayFrame::Data { dst, payload }) => {
                        if dst == node {
                            continue;
                        }
                        if !state.relay.send_from(
                            node, conn_id, dst,
                            &RelayFrame::Deliver {
                                src: node,
                                payload,
                            },
                        ) {
                            if let Ok(frame) =
                                RelayFrame::Error("destino offline".into()).encode()
                            {
                                let _ = tx.try_send(frame);
                            }
                        }
                    }
                    Ok(RelayFrame::Ping) => {
                        if let Ok(frame) = RelayFrame::Pong.encode() {
                            let _ = tx.try_send(frame);
                        }
                    }
                    Ok(_) => {}
                    Err(err) => {
                        tracing::debug!(%err, %node, "relay: frame inválido");
                    }
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    state.relay.unregister(node, conn_id);
    tracing::info!(%node, "relay: desconectado");
}


// Fail closed on database failures as well as disabled/deleted nodes.
async fn node_is_active(db: &sqlx::SqlitePool, node: NodeId) -> bool {
    matches!(db::node_by_id(db, node).await, Ok(Some(n)) if n.status() == NodeStatus::Active)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn upgrade_reauthorization_rejects_revoked_missing_and_db_failure() {
        let dir = tempfile::tempdir().unwrap();
        let pool = db::open(&dir.path().join("relay.sqlite")).await.unwrap();
        let node = db::create_node(
            &pool, "relay", "host", "test", "test", "test", "key", &[],
            &Default::default(), NodeStatus::Active, "test", "hash",
            "100.64.0.0/24".parse().unwrap(),
        ).await.unwrap();
        let id = uuid::Uuid::parse_str(&node.id).unwrap();
        assert!(node_is_active(&pool, id).await);
        // Models a valid token upgraded after its node was revoked.
        db::set_node_status(&pool, id, NodeStatus::Disabled).await.unwrap();
        assert!(!node_is_active(&pool, id).await);
        assert!(!node_is_active(&pool, uuid::Uuid::new_v4()).await);
        pool.close().await;
        assert!(!node_is_active(&pool, id).await);
    }
}
