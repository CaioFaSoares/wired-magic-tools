//! Relay WebSocket client. One connection per node carries every relayed peer.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use http::header::AUTHORIZATION;
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use wm_proto::{RelayFrame, RelayInfo};

use crate::agent::Cmd;

pub(crate) fn spawn(
    mut info_rx: watch::Receiver<Option<RelayInfo>>,
    mut out_rx: mpsc::Receiver<Vec<u8>>,
    cmd_tx: mpsc::Sender<Cmd>,
    connected: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut backoff = Duration::from_secs(1);
        loop {
            let info = info_rx.borrow().clone();
            let Some(info) = info else {
                connected.store(false, Ordering::Relaxed);
                if info_rx.changed().await.is_err() {
                    return;
                }
                continue;
            };
            if info.url.trim().is_empty() {
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
            match connect(&info.url, &info.token).await {
                Ok((mut sink, mut stream)) => {
                    tracing::info!(url = %info.url, "relay conectado");
                    connected.store(true, Ordering::Relaxed);
                    let _ = cmd_tx.send(Cmd::RelayConnected(true)).await;
                    backoff = Duration::from_secs(1);
                    let mut ping = tokio::time::interval(Duration::from_secs(20));
                    loop {
                        tokio::select! {
                            _ = ping.tick() => {
                                if sink.send(Message::Binary(RelayFrame::Ping.encode().unwrap_or_default().into())).await.is_err() {
                                    break;
                                }
                            }
                            out = out_rx.recv() => {
                                match out {
                                    Some(bytes) => {
                                        if sink.send(Message::Binary(bytes.into())).await.is_err() {
                                            break;
                                        }
                                    }
                                    None => { connected.store(false, Ordering::Relaxed); return; }
                                }
                            }
                            incoming = stream.next() => {
                                let Some(msg) = incoming else { break };
                                match msg {
                                    Ok(Message::Binary(bytes)) => match RelayFrame::decode(&bytes) {
                                        Ok(RelayFrame::Deliver { src, payload }) => {
                                            let _ = cmd_tx.send(Cmd::Relay { from: src, data: payload }).await;
                                        }
                                        Ok(RelayFrame::Presence { node, online }) => {
                                            let _ = cmd_tx.send(Cmd::RelayOnline { node, online }).await;
                                        }
                                        Ok(RelayFrame::Ping) => {
                                            let _ = sink.send(Message::Binary(RelayFrame::Pong.encode().unwrap_or_default().into())).await;
                                        }
                                        Ok(RelayFrame::Error(message)) => {
                                            tracing::warn!(%message, "relay erro");
                                        }
                                        Ok(_) => {}
                                        Err(err) => tracing::debug!(%err, "relay frame inválido"),
                                    },
                                    Ok(Message::Ping(payload)) => {
                                        let _ = sink.send(Message::Pong(payload)).await;
                                    }
                                    Ok(Message::Close(_)) | Err(_) => break,
                                    _ => {}
                                }
                            }
                            changed = info_rx.changed() => {
                                if changed.is_err() {
                                    connected.store(false, Ordering::Relaxed);
                                    return;
                                }
                                let next = info_rx.borrow().clone();
                                let next_url = next.as_ref().map(|i| i.url.clone());
                                if next_url.as_deref() != Some(info.url.as_str()) {
                                    break;
                                }
                            }
                        }
                    }
                    connected.store(false, Ordering::Relaxed);
                    let _ = cmd_tx.send(Cmd::RelayConnected(false)).await;
                    tracing::warn!("relay desconectado; tentando novamente");
                }
                Err(err) => {
                    connected.store(false, Ordering::Relaxed);
                    tracing::warn!(%err, "falha ao conectar no relay");
                    let _ = cmd_tx.send(Cmd::RelayConnected(false)).await;
                }
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
    })
}

type WsStream = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

async fn connect(
    url: &str,
    token: &str,
) -> anyhow::Result<(
    futures_util::stream::SplitSink<WsStream, Message>,
    futures_util::stream::SplitStream<WsStream>,
)> {
    let ws_url = to_ws_url(url)?;
    let mut request = ws_url.clone().into_client_request()?;
    request.headers_mut().insert(
        AUTHORIZATION,
        format!("Bearer {token}").parse().map_err(|_| anyhow::anyhow!("token inválido"))?,
    );
    let (ws, _resp) = connect_async(request).await?;
    let (sink, stream) = ws.split();
    Ok((sink, stream))
}

pub fn to_ws_url(url: &str) -> anyhow::Result<String> {
    let trimmed = url.trim_end_matches('/');
    if let Some(rest) = trimmed.strip_prefix("https://") {
        Ok(format!("wss://{rest}"))
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        Ok(format!("ws://{rest}"))
    } else if trimmed.starts_with("wss://") || trimmed.starts_with("ws://") {
        Ok(trimmed.to_string())
    } else {
        Err(anyhow::anyhow!("URL de relay inválida: {url}"))
    }
}
