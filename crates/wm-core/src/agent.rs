//! Agent daemon: control plane polling, data plane, discovery, relay and local API.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Context;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use tokio::net::UdpSocket;
use tokio::sync::{RwLock, mpsc, oneshot, watch};
use tokio::time::MissedTickBehavior;
use uuid::Uuid;
use x25519_dalek::StaticSecret;

use wm_proto::{
    AgentStatus, CompiledAcl, ControlEvent, EndpointCandidate, EndpointKind, EndpointsRequest,
    ExtensionDesired, ExtensionStatus, NatType, NetworkMap, NodeId, PathKind, PeerInfo, PeerStats,
    PeerStatus, PingResult, RelayFrame, RelayInfo, ReportRequest, SelfInfo, ShareInfo,
};

use crate::control::{ControlClient, ControlError};
use crate::local;
use crate::packet;
use crate::relay_client;
use crate::state::{self, AgentConfig, NodeState};
use crate::stun;
use crate::tun::{self, TunFramed};
use crate::wireguard::{WgEngine, WgOut};

pub(crate) enum Cmd {
    Tun(Vec<u8>),
    Udp { data: Vec<u8>, from: SocketAddr },
    Relay { from: NodeId, data: Vec<u8> },
    Map(Box<NetworkMap>),
    Extensions(Vec<ExtensionDesired>),
    Control { connected: bool, error: Option<String> },
    RelayOnline { node: NodeId, online: bool },
    RelayConnected(bool),
    NatType(NatType),
    Timer,
    Ping {
        peer: String,
        reply: oneshot::Sender<Result<PingResult, String>>,
    },
    Reconnect {
        peer: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
}

#[derive(Clone)]
pub struct AgentSnapshot {
    pub status: AgentStatus,
    pub peers: Vec<PeerStatus>,
    pub shares: Vec<ShareInfo>,
    pub peer_ips: BTreeMap<String, (Uuid, Ipv4Addr)>,
    pub self_name: String,
    pub self_ip: Ipv4Addr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct FlowKey {
    src: Ipv4Addr,
    dst: Ipv4Addr,
    sport: u16,
    dport: u16,
    proto: u8,
}

impl FlowKey {
    fn from_packet(meta: &packet::PacketMeta, pkt: &[u8]) -> Option<Self> {
        match meta.protocol {
            6 | 17 => Some(Self {
                src: meta.src,
                dst: meta.dst,
                sport: meta.src_port?,
                dport: meta.dst_port?,
                proto: meta.protocol,
            }),
            1 if matches!(meta.icmp_type, Some(0) | Some(8)) => {
                let (id, seq) = packet::icmp_id_seq(pkt)?;
                Some(Self {
                    src: meta.src,
                    dst: meta.dst,
                    sport: id,
                    dport: seq,
                    proto: 1,
                })
            }
            _ => None,
        }
    }

    fn reverse(self) -> Self {
        if self.proto == 1 {
            // ICMP echo request/reply share the same id and sequence.
            Self {
                src: self.dst,
                dst: self.src,
                sport: self.sport,
                dport: self.dport,
                proto: 1,
            }
        } else {
            Self {
                src: self.dst,
                dst: self.src,
                sport: self.dport,
                dport: self.sport,
                proto: self.proto,
            }
        }
    }
}

#[derive(Clone, Copy)]
struct FlowEntry {
    seen: Instant,
    // Preserve the initiating direction; replies must not authorize the reverse flow.
    origin: FlowKey,
}

fn revalidate_flows(flows: &mut HashMap<FlowKey, FlowEntry>, acl: &CompiledAcl) {
    flows.retain(|_, entry| {
        let key = entry.origin;
        acl.is_allowed(key.src, key.dst, key.proto, if key.proto == 1 { None } else { Some(key.dport) })
    });
}

fn record_flow(flows: &mut HashMap<FlowKey, FlowEntry>, key: FlowKey) {
    if flows.len() > 4096 {
        let cutoff = Instant::now() - Duration::from_secs(120);
        flows.retain(|_, entry| entry.seen > cutoff);
        if flows.len() > 4096 {
            flows.clear();
        }
    }
    let now = Instant::now();
    let origin = flows.get(&key).map(|entry| entry.origin).unwrap_or(key);
    let entry = FlowEntry { seen: now, origin };
    flows.insert(key, entry);
    flows.insert(key.reverse(), entry);
}

struct PendingPing {
    started: Instant,
    reply: oneshot::Sender<Result<PingResult, String>>,
}

struct DataPlane {
    state: NodeState,
    engine: WgEngine,
    acl: CompiledAcl,
    socket: Arc<UdpSocket>,
    tun: SplitSink<TunFramed, Vec<u8>>,
    relay_out: mpsc::Sender<Vec<u8>>,
    relay_info_tx: watch::Sender<Option<RelayInfo>>,
    relay_connected: Arc<AtomicBool>,
    extensions_tx: watch::Sender<Vec<ExtensionDesired>>,
    base_exposes: Vec<state::ExposeSpec>,
    extension_exposes: Vec<state::ExposeSpec>,
    expose_tx: watch::Sender<Vec<state::ExposeSpec>>,
    shares: Vec<ShareInfo>,
    state_path: PathBuf,
    relay_online: HashSet<NodeId>,
    endpoint_cache: HashMap<SocketAddr, NodeId>,
    ip_to_peer: HashMap<Ipv4Addr, NodeId>,
    snapshot: Arc<RwLock<AgentSnapshot>>,
    started: Instant,
    map_version: u64,
    control_connected: bool,
    last_error: Option<String>,
    tun_name: String,
    udp_port: u16,
    nat_type: NatType,
    last_snapshot: Instant,
    pending_pings: HashMap<(u16, u16), PendingPing>,
    flows: HashMap<FlowKey, FlowEntry>,
    force_relay: bool,
}

impl DataPlane {
    async fn on_map(&mut self, map: NetworkMap) {
        let key_ok = map.me.wg_public_key == self.engine.local_public();
        let _ = self.extensions_tx.send(map.extensions.clone());
        self.shares = map.shares.clone();
        if self.state.node_name != map.me.name {
            self.state.node_name = map.me.name.clone();
            if let Err(err) = self.state.save(&self.state_path) {
                tracing::warn!(%err, "não foi possível persistir o novo nome do nó");
            }
        }
        self.engine.apply_peers(&map.peers);
        self.acl = map.acl;
        revalidate_flows(&mut self.flows, &self.acl);
        self.control_connected = true;
        self.map_version = map.version;
        self.last_error = if key_ok {
            None
        } else {
            Some("a chave WireGuard local não confere com o mapa; reenrole este nó".into())
        };
        let _ = self.relay_info_tx.send(map.relay.clone());
        self.rebuild_ip_map();
        tracing::info!(
            version = map.version,
            peers = map.peers.len(),
            "mapa atualizado"
        );
        let ids: Vec<NodeId> = self.engine.peers().map(|p| p.id).collect();
        for id in ids {
            self.kick(id).await;
        }
        self.update_snapshot().await;
    }

    fn rebuild_ip_map(&mut self) {
        self.ip_to_peer.clear();
        for peer in self.engine.peers() {
            self.ip_to_peer.insert(peer.overlay_ip, peer.id);
        }
    }

    async fn kick(&mut self, id: NodeId) {
        let Some(peer) = self.engine.peer(id) else {
            return;
        };
        if self.force_relay {
            let relay_ok = self.relay_connected.load(Ordering::Relaxed) && peer.relay_online;
            if !relay_ok {
                return;
            }
            if self.engine.handshake_age(id).is_some_and(|age| age < Duration::from_secs(120)) {
                return;
            }
            self.engine.set_path(id, PathKind::Relay);
            let out = self.engine.initiate(id);
            self.handle_out(id, out, None).await;
            return;
        }
        let age = self.engine.handshake_age(id);
        let has_endpoints = !peer.endpoints.is_empty();
        let relay_online = peer.relay_online;
        let relay_connected = self.relay_connected.load(Ordering::Relaxed);
        if age.is_some_and(|age| age < Duration::from_secs(120)) {
            return;
        }
        if !has_endpoints && !(relay_connected && relay_online) {
            return;
        }
        if relay_connected && relay_online && !has_endpoints {
            self.engine.set_path(id, PathKind::Relay);
        }
        let out = self.engine.initiate(id);
        self.handle_out(id, out, None).await;
    }

    async fn on_tun(&mut self, packet: Vec<u8>) {
        let Some(meta) = packet::parse(&packet) else {
            return;
        };
        let Some(id) = self.ip_to_peer.get(&meta.dst).copied() else {
            return;
        };
        let flow = FlowKey::from_packet(&meta, &packet);
        let established = flow.is_some_and(|key| self.flows.contains_key(&key));
        if !established
            && !self.acl.is_allowed(
                self.state.overlay_ip,
                meta.dst,
                meta.protocol,
                meta.dst_port,
            )
        {
            tracing::debug!(dst = %meta.dst, port = ?meta.dst_port, "ACL de saída bloqueou pacote");
            return;
        }
        if let Some(key) = flow {
            if meta.protocol != 1 || meta.icmp_type == Some(8) {
                self.learn_flow(key);
            }
        }
        let out = self.engine.encapsulate(id, &packet);
        self.handle_out(id, out, None).await;
    }

    async fn on_udp(&mut self, data: Vec<u8>, from: SocketAddr) {
        if self.force_relay {
            return;
        }
        let preferred = self.endpoint_cache.get(&from).copied();
        let outs = self.engine.receive(&data, Some(from.ip()), preferred);
        for (id, out) in outs {
            if !matches!(out, WgOut::Err(_)) {
                self.endpoint_cache.insert(from, id);
                self.engine.set_endpoint(id, from);
                self.engine.set_path(id, PathKind::Direct);
            }
            self.handle_out(id, out, Some(from)).await;
        }
    }

    async fn on_relay(&mut self, from: NodeId, data: Vec<u8>) {
        let out = self.engine.decapsulate(from, None, &data);
        if !matches!(out, WgOut::Err(_)) {
            self.relay_online.insert(from);
            self.engine.set_relay_online(from, true);
            if self.engine.peer(from).map(|p| p.path) != Some(PathKind::Direct) {
                self.engine.set_path(from, PathKind::Relay);
            }
        }
        self.handle_out(from, out, None).await;
    }

    async fn handle_out(&mut self, id: NodeId, out: WgOut, target: Option<SocketAddr>) {
        match out {
            WgOut::Network(data) => {
                self.send_network(id, data, target).await;
                self.drain_peer(id, target).await;
            }
            WgOut::Tunnel(data) => {
                self.ingress(id, &data, target.is_some()).await;
                self.drain_peer(id, target).await;
            }
            WgOut::Done => {}
            WgOut::Err(err) => {
                self.last_error = Some(err.clone());
                if let Some(peer) = self.engine.peer_mut(id) {
                    peer.last_error = Some(err);
                }
            }
        }
    }

    async fn drain_peer(&mut self, id: NodeId, target: Option<SocketAddr>) {
        for _ in 0..64 {
            let outs = self.engine.drain(id);
            if outs.is_empty() {
                break;
            }
            for out in outs {
                match out {
                    WgOut::Network(data) => self.send_network(id, data, target).await,
                    WgOut::Tunnel(data) => self.ingress(id, &data, target.is_some()).await,
                    WgOut::Err(err) => {
                        self.last_error = Some(err);
                        break;
                    }
                    WgOut::Done => {}
                }
            }
        }
    }

    async fn send_network(&mut self, id: NodeId, data: Vec<u8>, target: Option<SocketAddr>) {
        if let Some(target) = target {
            if self.socket.send_to(&data, target).await.is_ok() {
                self.endpoint_cache.insert(target, id);
                self.engine.set_endpoint(id, target);
                self.engine.set_path(id, PathKind::Direct);
            } else {
                self.last_error = Some(format!("falha ao enviar UDP para {target}"));
            }
            return;
        }

        if self.force_relay {
            if self.relay_connected.load(Ordering::Relaxed) {
                self.send_relay(id, &data);
            }
            return;
        }
        let is_init = data.len() >= 4 && u32::from_le_bytes(data[..4].try_into().unwrap()) == 1;
        let Some(peer) = self.engine.peer(id) else {
            return;
        };
        let path = peer.path;
        let endpoint = peer.endpoint;
        let endpoints = peer.endpoints.clone();
        let relay_online = peer.relay_online;
        let relay_connected = self.relay_connected.load(Ordering::Relaxed);

        if is_init {
            let mut sent_any = false;
            for ep in &endpoints {
                if self.socket.send_to(&data, *ep).await.is_ok() {
                    sent_any = true;
                }
            }
            if sent_any {
                if let Some(peer) = self.engine.peer_mut(id) {
                    peer.last_direct_attempt = Some(Instant::now());
                }
            }
            if relay_connected && (relay_online || path == PathKind::Relay || !sent_any) {
                self.send_relay(id, &data);
            }
            return;
        }

        if path == PathKind::Direct {
            if let Some(endpoint) = endpoint {
                if self.socket.send_to(&data, endpoint).await.is_ok() {
                    return;
                }
                self.last_error = Some(format!("UDP direto para {endpoint} falhou"));
            }
        }
        if relay_connected && (relay_online || path == PathKind::Relay) {
            if self.send_relay(id, &data) {
                return;
            }
        }
        if let Some(ep) = endpoints.first().copied() {
            if self.socket.send_to(&data, ep).await.is_ok() {
                if let Some(peer) = self.engine.peer_mut(id) {
                    peer.last_direct_attempt = Some(Instant::now());
                }
            }
        }
    }

    fn send_relay(&self, id: NodeId, data: &[u8]) -> bool {
        if !self.relay_connected.load(Ordering::Relaxed) {
            return false;
        }
        let Ok(frame) = RelayFrame::Data {
            dst: id,
            payload: data.to_vec(),
        }
        .encode()
        else {
            return false;
        };
        self.relay_out.try_send(frame).is_ok()
    }

    async fn ingress(&mut self, id: NodeId, data: &[u8], direct: bool) {
        let Some(meta) = packet::parse(data) else {
            return;
        };
        let Some(peer) = self.engine.peer(id) else {
            return;
        };
        if meta.src != peer.overlay_ip || meta.dst != self.state.overlay_ip {
            tracing::debug!(from = %peer.name, src = %meta.src, dst = %meta.dst, "pacote com IP inesperado; drop");
            return;
        }
        let peer_name = peer.name.clone();
        let path = peer.path;
        let flow = FlowKey::from_packet(&meta, data);
        let established = flow.is_some_and(|key| self.flows.contains_key(&key));
        if !established
            && !self
                .acl
                .is_allowed(meta.src, meta.dst, meta.protocol, meta.dst_port)
        {
            tracing::debug!(from = %peer_name, port = ?meta.dst_port, "ACL de entrada bloqueou pacote");
            return;
        }
        if let Some(key) = flow {
            if meta.protocol != 1 || meta.icmp_type == Some(8) {
                self.learn_flow(key);
            }
        }

        if meta.is_icmp_echo_reply() {
            if let Some((id, seq)) = packet::icmp_id_seq(data) {
                if let Some(pending) = self.pending_pings.remove(&(id, seq)) {
                    let rtt = pending.started.elapsed().as_secs_f64() * 1000.0;
                    let _ = pending.reply.send(Ok(PingResult {
                        peer: peer_name.clone(),
                        overlay_ip: meta.src,
                        sent: 1,
                        received: 1,
                        rtt_ms_min: Some(rtt),
                        rtt_ms_avg: Some(rtt),
                        rtt_ms_max: Some(rtt),
                        path,
                    }));
                    return;
                }
            }
        }

        if let Err(err) = self.tun.send(data.to_vec()).await {
            self.last_error = Some(format!("escrita no TUN falhou: {err}"));
            return;
        }
        if direct {
            if let Some(src) = self.endpoint_cache.iter().find(|(_, v)| **v == id).map(|(k, _)| *k)
            {
                self.engine.set_endpoint(id, src);
            }
            self.engine.set_path(id, PathKind::Direct);
        } else if self.engine.peer(id).map(|p| p.path) != Some(PathKind::Direct) {
            self.engine.set_path(id, PathKind::Relay);
        }
    }

    fn publish_exposes(&self) {
        let mut merged: Vec<state::ExposeSpec> = self.base_exposes.clone();
        let mut seen: HashSet<u16> = merged.iter().map(|spec| spec.overlay_port).collect();
        for expose in &self.extension_exposes {
            if seen.insert(expose.overlay_port) {
                merged.push(*expose);
            }
        }
        let _ = self.expose_tx.send(merged);
    }

    async fn on_timer(&mut self) {
        let ids: Vec<NodeId> = self.engine.peers().map(|p| p.id).collect();
        let relay_connected = self.relay_connected.load(Ordering::Relaxed);
        for id in ids {
            let out = self.engine.update_timers(id);
            self.handle_out(id, out, None).await;

            let Some(peer) = self.engine.peer(id) else {
                continue;
            };
            let age = self.engine.handshake_age(id);
            let has_endpoints = !peer.endpoints.is_empty();
            let last_direct = peer.last_direct_attempt;
            let last_initiate = peer.last_initiate;
            let path = peer.path;
            let relay_online = peer.relay_online;

            if self.force_relay {
                if (age.is_none() || age.is_some_and(|a| a > Duration::from_secs(120)))
                    && relay_connected
                    && relay_online
                    && last_initiate
                        .map(|t| t.elapsed() > Duration::from_secs(5))
                        .unwrap_or(true)
                {
                    self.engine.set_path(id, PathKind::Relay);
                    let out = self.engine.initiate(id);
                    self.handle_out(id, out, None).await;
                }
                continue;
            }
            if age.is_none() || age.is_some_and(|a| a > Duration::from_secs(120)) {
                let direct_due = last_direct
                    .map(|t| t.elapsed() > Duration::from_secs(3))
                    .unwrap_or(true);
                if has_endpoints && direct_due {
                    let out = self.engine.initiate(id);
                    self.handle_out(id, out, None).await;
                } else if relay_connected && relay_online {
                    let relay_due = last_initiate
                        .map(|t| t.elapsed() > Duration::from_secs(5))
                        .unwrap_or(true);
                    if relay_due {
                        self.engine.set_path(id, PathKind::Relay);
                        let out = self.engine.initiate(id);
                        self.handle_out(id, out, None).await;
                    }
                }
            } else if path == PathKind::Relay
                && has_endpoints
                && last_direct
                    .map(|t| t.elapsed() > Duration::from_secs(30))
                    .unwrap_or(true)
            {
                let out = self.engine.initiate(id);
                self.handle_out(id, out, None).await;
            }
        }

        self.flows
            .retain(|_, entry| entry.seen.elapsed() < Duration::from_secs(300));

        let expired: Vec<(u16, u16)> = self
            .pending_pings
            .iter()
            .filter(|(_, p)| p.started.elapsed() > Duration::from_secs(5))
            .map(|(k, _)| *k)
            .collect();
        for key in expired {
            if let Some(p) = self.pending_pings.remove(&key) {
                let _ = p.reply.send(Err("ping sem resposta".into()));
            }
        }

        if self.last_snapshot.elapsed() > Duration::from_secs(1) {
            self.update_snapshot().await;
            self.last_snapshot = Instant::now();
        }
    }

    async fn on_ping(&mut self, peer: String, reply: oneshot::Sender<Result<PingResult, String>>) {
        let Some(id) = self.find_peer(&peer) else {
            let _ = reply.send(Err(format!("nó '{peer}' não encontrado")));
            return;
        };
        let Some(p) = self.engine.peer(id) else {
            let _ = reply.send(Err("peer sem estado".into()));
            return;
        };
        let dst = p.overlay_ip;
        let name = p.name.clone();
        let path = p.path;
        if !self
            .acl
            .is_allowed(self.state.overlay_ip, dst, 1, None)
        {
            let _ = reply.send(Err("ACL bloqueia ICMP para este nó".into()));
            return;
        }
        let mut id_num = [0u8; 2];
        if getrandom::fill(&mut id_num).is_err() {
            let _ = reply.send(Err("entropia indisponível".into()));
            return;
        }
        let icmp_id = u16::from_be_bytes(id_num);
        let seq = 1u16;
        let request = packet::icmp_echo_request(self.state.overlay_ip, dst, icmp_id, seq);
        self.learn_flow(FlowKey {
            src: self.state.overlay_ip,
            dst,
            sport: icmp_id,
            dport: seq,
            proto: 1,
        });
        self.pending_pings.insert(
            (icmp_id, seq),
            PendingPing {
                started: Instant::now(),
                reply,
            },
        );
        let out = self.engine.encapsulate(id, &request);
        self.handle_out(id, out, None).await;
        tracing::debug!(%name, %dst, path = %path, "ping enviado");
    }

    async fn on_reconnect(
        &mut self,
        peer: String,
        reply: oneshot::Sender<Result<(), String>>,
    ) {
        let Some(id) = self.find_peer(&peer) else {
            let _ = reply.send(Err(format!("nó '{peer}' não encontrado")));
            return;
        };
        self.engine.reset_path(id);
        let out = self.engine.initiate(id);
        self.handle_out(id, out, None).await;
        let _ = reply.send(Ok(()));
    }

    fn find_peer(&self, peer: &str) -> Option<NodeId> {
        if let Ok(id) = Uuid::parse_str(peer) {
            if self.engine.peer(id).is_some() {
                return Some(id);
            }
        }
        self.engine.peer_by_name(peer).map(|p| p.id)
    }

    async fn on_relay_online(&mut self, node: NodeId, online: bool) {
        self.engine.set_relay_online(node, online);
        if online {
            self.relay_online.insert(node);
            if self.engine.peer(node).map(|p| p.path) == Some(PathKind::None) {
                let out = self.engine.initiate(node);
                self.handle_out(node, out, None).await;
            }
        } else {
            self.relay_online.remove(&node);
            if self.engine.peer(node).map(|p| p.path) == Some(PathKind::Relay) {
                self.engine.set_path(node, PathKind::None);
            }
        }
    }

    async fn on_relay_connected(&mut self, connected: bool) {
        if !connected {
            self.relay_online.clear();
            let ids: Vec<NodeId> = self.engine.peers().map(|p| p.id).collect();
            for id in ids {
                self.engine.set_relay_online(id, false);
                if self.engine.peer(id).map(|p| p.path) == Some(PathKind::Relay) {
                    self.engine.set_path(id, PathKind::None);
                }
            }
        }
        self.update_snapshot().await;
    }

    fn learn_flow(&mut self, key: FlowKey) {
        record_flow(&mut self.flows, key);
    }

    async fn update_snapshot(&mut self) {
        let mut peers: Vec<PeerStatus> = Vec::new();
        for peer in self.engine.peers() {
            let stats = self.engine.stats(peer.id);
            peers.push(PeerStatus {
                id: peer.id,
                name: peer.name.clone(),
                overlay_ip: peer.overlay_ip,
                path: peer.path,
                endpoint: peer.endpoint.map(|e| e.to_string()),
                rtt_ms: stats.as_ref().and_then(|s| s.3),
                last_handshake_secs: stats.as_ref().and_then(|s| s.0).map(|d| d.as_secs()),
                rx_bytes: stats.as_ref().map(|s| s.2).unwrap_or(0),
                tx_bytes: stats.as_ref().map(|s| s.1).unwrap_or(0),
                relay_online: peer.relay_online,
                last_error: peer.last_error.clone(),
            });
        }
        peers.sort_by_key(|p| p.overlay_ip);
        let peers_online = peers
            .iter()
            .filter(|p| p.path != PathKind::None || p.relay_online)
            .count();
        let status = AgentStatus {
            version: env!("CARGO_PKG_VERSION").to_string(),
            node_id: Some(self.state.node_id),
            node_name: Some(self.state.node_name.clone()),
            overlay_ip: Some(self.state.overlay_ip),
            control_url: Some(self.state.control_url.clone()),
            control_connected: self.control_connected,
            relay_connected: self.relay_connected.load(Ordering::Relaxed),
            tun_name: Some(self.tun_name.clone()),
            udp_port: Some(self.udp_port),
            nat_type: self.nat_type,
            peers_total: peers.len(),
            peers_online,
            uptime_secs: self.started.elapsed().as_secs(),
            map_version: self.map_version,
            last_error: self.last_error.clone(),
        };
        let mut peer_ips = BTreeMap::new();
        for p in &peers {
            peer_ips.insert(p.name.clone(), (p.id, p.overlay_ip));
        }
        *self.snapshot.write().await = AgentSnapshot {
            status,
            peers,
            shares: self.shares.clone(),
            peer_ips,
            self_name: self.state.node_name.clone(),
            self_ip: self.state.overlay_ip,
        };
    }
}

pub async fn run(cfg: AgentConfig) -> anyhow::Result<()> {
    let (tx, rx) = oneshot::channel();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = tx.send(());
    });
    run_until(cfg, rx).await
}

pub async fn run_until(cfg: AgentConfig, shutdown: oneshot::Receiver<()>) -> anyhow::Result<()> {
    let state = state::require_state(&cfg)?;
    let control_url = cfg
        .control_url
        .clone()
        .unwrap_or_else(|| state.control_url.clone());
    let control_url = crate::control::parse_control_url(&control_url)?;
    let control = ControlClient::new(&control_url)?;

    cfg.data_dir_ensure()?;
    // Node-side hook assets (scripts) are bundled with the binary. The control
    // seeds its own data dir; the agent must have them too, including on remote
    // nodes that never share a filesystem with the control.
    if let Err(err) = wm_ext::seed_builtin_manifests(&cfg.data_dir.join("extensions")) {
        tracing::warn!(%err, "não foi possível semear assets de extensões no nó");
    }
    let pid_path = cfg.pid_path();
    std::fs::write(&pid_path, format!("{}\n", std::process::id()))?;

    let result = run_inner(cfg, state, control, pid_path.clone(), shutdown).await;
    let _ = std::fs::remove_file(&pid_path);
    result
}

impl AgentConfig {
    fn data_dir_ensure(&self) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.data_dir)
            .with_context(|| format!("criando {}", self.data_dir.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.data_dir, std::fs::Permissions::from_mode(0o700));
        }
        Ok(())
    }
}

async fn run_inner(
    cfg: AgentConfig,
    state: NodeState,
    control: ControlClient,
    _pid_path: PathBuf,
    shutdown: oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    let socket = Arc::new(
        UdpSocket::bind(("0.0.0.0", cfg.udp_port))
            .await
            .with_context(|| format!("bind UDP 0.0.0.0:{}", cfg.udp_port))?,
    );
    let (tun_dev, _route_guard) = tun::create(&cfg, state.overlay_ip, state.overlay_cidr)?;
    let tun_name = tun_dev.name.clone();
    let (tun_sink, tun_stream) = tun_dev.framed.split();

    let local_secret = StaticSecret::from(
        <[u8; 32]>::try_from(
            STANDARD
                .decode(&state.wg_private_key)
                .context("chave privada do estado inválida")?,
        )
        .map_err(|_| anyhow::anyhow!("chave privada do estado inválida"))?,
    );
    let engine = WgEngine::new(local_secret);

    let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>(8192);
    let (stun_tx, stun_rx) = mpsc::channel::<(Vec<u8>, SocketAddr)>(64);
    let (relay_info_tx, relay_info_rx) = watch::channel::<Option<RelayInfo>>(None);
    let (relay_out_tx, relay_out_rx) = mpsc::channel::<Vec<u8>>(2048);
    let relay_connected = Arc::new(AtomicBool::new(false));
    let snapshot = Arc::new(RwLock::new(initial_snapshot(&state, &tun_name, cfg.udp_port)));
    let (stun_urls_tx, stun_urls_rx) = watch::channel::<Vec<String>>(Vec::new());
    let expose_specs = if cfg.expose.is_empty() {
        state.expose.clone()
    } else {
        cfg.expose.clone()
    };
    let overlay_ip = state.overlay_ip;
    let (expose_tx, expose_rx) = watch::channel(expose_specs.clone());
    let (extensions_tx, extensions_rx) = watch::channel::<Vec<ExtensionDesired>>(Vec::new());
    let (extension_status_tx, extension_status_rx) =
        watch::channel::<Vec<ExtensionStatus>>(Vec::new());

    let mut tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    tasks.push(tokio::spawn(udp_loop(socket.clone(), cmd_tx.clone(), stun_tx)));
    tasks.push(tokio::spawn(tun_loop(tun_stream, cmd_tx.clone())));
    tasks.push(tokio::spawn(timer_loop(cmd_tx.clone())));
    tasks.push(tokio::spawn(map_loop(
        control.clone(),
        state.node_token.clone(),
        cmd_tx.clone(),
        stun_urls_tx,
    )));
    tasks.push(tokio::spawn(async move {
        if let Err(err) = crate::expose::run(overlay_ip, expose_rx).await {
            tracing::error!(%err, "forward de portas encerrou");
        }
    }));
    let hook_ctx = crate::extensions::hooks::HookContext {
        node_id: state.node_id,
        node_name: state.node_name.clone(),
        overlay_ip,
        control_url: state.control_url.clone(),
        data_dir: cfg.data_dir.clone(),
        extensions_dir: cfg.data_dir.join("extensions"),
    };
    tasks.push(tokio::spawn(crate::extensions::run(
        extensions_rx,
        extension_status_tx,
        cfg.data_dir.join("extensions"),
        hook_ctx,
    )));
    tasks.push(tokio::spawn(report_loop(
        control.clone(),
        state.node_token.clone(),
        snapshot.clone(),
        extension_status_rx.clone(),
    )));
    tasks.push(tokio::spawn(discovery_loop(
        socket.clone(),
        stun_rx,
        control.clone(),
        state.node_token.clone(),
        cfg.udp_port,
        stun_urls_rx,
        cmd_tx.clone(),
    )));
    tasks.push(relay_client::spawn(
        relay_info_rx,
        relay_out_rx,
        cmd_tx.clone(),
        relay_connected.clone(),
    ));
    let local_socket = cfg.socket_path();
    let local_snapshot = snapshot.clone();
    let local_cmd = cmd_tx.clone();
    let local_extensions = extension_status_rx;
    let local_control = ControlClient::new(control.base())?;
    let local_token = state.node_token.clone();
    let local_state_path = cfg.state_path();
    tasks.push(tokio::spawn(async move {
        if let Err(err) = local::serve(
            local_socket,
            local_snapshot,
            local_cmd,
            local_extensions,
            local_control,
            local_token,
            local_state_path,
        )
        .await
        {
            tracing::error!(%err, "API local encerrou");
        }
    }));

    let mut plane = DataPlane {
        state,
        engine,
        acl: CompiledAcl::default(),
        socket,
        tun: tun_sink,
        relay_out: relay_out_tx,
        relay_info_tx,
        relay_connected,
        extensions_tx,
        base_exposes: expose_specs,
        extension_exposes: Vec::new(),
        expose_tx,
        shares: Vec::new(),
        state_path: cfg.state_path(),
        relay_online: HashSet::new(),
        endpoint_cache: HashMap::new(),
        ip_to_peer: HashMap::new(),
        snapshot,
        started: Instant::now(),
        map_version: 0,
        control_connected: false,
        last_error: None,
        tun_name,
        udp_port: cfg.udp_port,
        nat_type: NatType::Unknown,
        last_snapshot: Instant::now(),
        pending_pings: HashMap::new(),
        flows: HashMap::new(),
        force_relay: cfg.force_relay,
    };

    tokio::select! {
        _ = data_loop(&mut plane, cmd_rx) => {}
        _ = shutdown => {
            tracing::info!("encerrando agente");
        }
    }

    plane.update_snapshot().await;
    for task in tasks {
        task.abort();
    }
    let _ = std::fs::remove_file(cfg.socket_path());
    let _ = std::fs::remove_file(cfg.pid_path());
    Ok(())
}

async fn data_loop(plane: &mut DataPlane, mut rx: mpsc::Receiver<Cmd>) {
    while let Some(cmd) = rx.recv().await {
        match cmd {
            Cmd::Tun(packet) => plane.on_tun(packet).await,
            Cmd::Udp { data, from } => plane.on_udp(data, from).await,
            Cmd::Relay { from, data } => plane.on_relay(from, data).await,
            Cmd::Map(map) => plane.on_map(*map).await,
            Cmd::Extensions(desired) => {
                let _ = plane.extensions_tx.send(desired);
                // Revoked/pending nodes must stop adapter-provided exposes.
                plane.extension_exposes.clear();
                plane.publish_exposes();
            }
            Cmd::Control { connected, error } => {
                plane.control_connected = connected;
                if error.is_some() {
                    plane.last_error = error;
                }
                plane.update_snapshot().await;
            }
            Cmd::RelayOnline { node, online } => plane.on_relay_online(node, online).await,
            Cmd::RelayConnected(connected) => plane.on_relay_connected(connected).await,
            Cmd::NatType(nat) => plane.nat_type = nat,
            Cmd::Timer => plane.on_timer().await,
            Cmd::Ping { peer, reply } => plane.on_ping(peer, reply).await,
            Cmd::Reconnect { peer, reply } => plane.on_reconnect(peer, reply).await,
        }
    }
}

async fn udp_loop(
    socket: Arc<UdpSocket>,
    cmd_tx: mpsc::Sender<Cmd>,
    stun_tx: mpsc::Sender<(Vec<u8>, SocketAddr)>,
) {
    let mut buf = vec![0u8; 2048];
    loop {
        match socket.recv_from(&mut buf).await {
            Ok((n, from)) => {
                let data = buf[..n].to_vec();
                if stun::is_stun(&data) {
                    let _ = stun_tx.try_send((data, from));
                } else if cmd_tx.send(Cmd::Udp { data, from }).await.is_err() {
                    return;
                }
            }
            Err(err) => {
                tracing::warn!(%err, "recv UDP");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

async fn tun_loop(
    mut stream: futures_util::stream::SplitStream<TunFramed>,
    cmd_tx: mpsc::Sender<Cmd>,
) {
    use futures_util::StreamExt;
    while let Some(pkt) = stream.next().await {
        match pkt {
            Ok(pkt) => {
                if cmd_tx.send(Cmd::Tun(pkt)).await.is_err() {
                    return;
                }
            }
            Err(err) => {
                tracing::warn!(%err, "leitura do TUN");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

async fn timer_loop(cmd_tx: mpsc::Sender<Cmd>) {
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        if cmd_tx.send(Cmd::Timer).await.is_err() {
            return;
        }
    }
}

async fn map_loop(
    control: ControlClient,
    token: String,
    cmd_tx: mpsc::Sender<Cmd>,
    stun_urls: watch::Sender<Vec<String>>,
) {
    let mut version = 0u64;
    let mut backoff = Duration::from_secs(1);
    loop {
        match control.map(&token, version, 25).await {
            Ok(map) => {
                version = map.version;
                let _ = stun_urls.send(map.stun.clone());
                if cmd_tx.send(Cmd::Map(Box::new(map))).await.is_err() {
                    return;
                }
                backoff = Duration::from_secs(1);
            }
            Err(err) => {
                let msg = match &err {
                    ControlError::Pending => "aguardando aprovação do operador".to_string(),
                    ControlError::Unauthorized => "token do nó inválido ou revogado".to_string(),
                    other => other.to_string(),
                };
                // A node that is pending or revoked must not keep running
                // sidecars. Network errors keep the last desired state, which
                // is what makes sidecars survive a control-plane outage.
                if matches!(err, ControlError::Pending | ControlError::Unauthorized) {
                    let _ = cmd_tx.send(Cmd::Extensions(Vec::new())).await;
                }
                tracing::warn!(error = %msg, "mapa indisponível");
                let _ = cmd_tx
                    .send(Cmd::Control {
                        connected: false,
                        error: Some(msg),
                    })
                    .await;
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        }
    }
}

async fn report_loop(
    control: ControlClient,
    token: String,
    snapshot: Arc<RwLock<AgentSnapshot>>,
    mut extension_status: watch::Receiver<Vec<ExtensionStatus>>,
) {
    tokio::time::sleep(Duration::from_secs(5)).await;
    let mut interval = tokio::time::interval(Duration::from_secs(30));
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut watch_alive = true;
    loop {
        tokio::select! {
            _ = interval.tick() => {}
            changed = extension_status.changed(), if watch_alive => {
                match changed {
                    Ok(()) => {
                        // Coalesce the starting/running/URL burst from a sidecar.
                        tokio::time::sleep(Duration::from_millis(750)).await;
                        extension_status.borrow_and_update();
                    }
                    Err(_) => watch_alive = false,
                }
            }
        }
        let (peers, nat_type, relay_connected) = {
            let snap = snapshot.read().await;
            let peers = snap
                .peers
                .iter()
                .map(|p| PeerStats {
                    peer_id: p.id,
                    path: p.path,
                    rtt_ms: p.rtt_ms,
                    last_handshake_secs: p.last_handshake_secs,
                    rx_bytes: p.rx_bytes,
                    tx_bytes: p.tx_bytes,
                    relay_online: p.relay_online,
                    endpoint: p.endpoint.clone(),
                    last_error: p.last_error.clone(),
                })
                .collect::<Vec<_>>();
            (peers, snap.status.nat_type, snap.status.relay_connected)
        };
        let extensions = extension_status.borrow().clone();
        let req = ReportRequest {
            peers,
            nat_type,
            relay_connected,
            extensions,
        };
        if let Err(err) = control.post_report(&token, &req).await {
            tracing::debug!(%err, "relatório não enviado");
        }
    }
}

async fn discovery_loop(
    socket: Arc<UdpSocket>,
    mut stun_rx: mpsc::Receiver<(Vec<u8>, SocketAddr)>,
    control: ControlClient,
    token: String,
    udp_port: u16,
    stun_urls: watch::Receiver<Vec<String>>,
    cmd_tx: mpsc::Sender<Cmd>,
) {
    loop {
        let urls = stun_urls.borrow().clone();
        let mut candidates: Vec<EndpointCandidate> = Vec::new();
        let mut mapped_ok = false;
        for url in &urls {
            match stun::resolve_stun_url(url).await {
                Ok(server) => match stun::discover(&socket, server, &mut stun_rx).await {
                    Ok(mapped) => {
                        candidates.push(EndpointCandidate {
                            addr: mapped,
                            kind: EndpointKind::Stun,
                        });
                        mapped_ok = true;
                        tracing::debug!(%mapped, %url, "STUN descoberto");
                        break;
                    }
                    Err(err) => tracing::debug!(%err, %url, "STUN falhou"),
                },
                Err(err) => tracing::debug!(%err, %url, "STUN inválido"),
            }
        }
        for ip in local_ipv4s() {
            candidates.push(EndpointCandidate {
                addr: SocketAddr::from((ip, udp_port)),
                kind: EndpointKind::Lan,
            });
        }
        dedupe(&mut candidates);
        let req = EndpointsRequest {
            udp_port,
            candidates,
            nat_type: if mapped_ok { NatType::Cone } else { NatType::Unknown },
            relay_connected: false,
        };
        if let Err(err) = control.post_endpoints(&token, &req).await {
            tracing::debug!(%err, "endpoints não publicados");
        }
        let _ = cmd_tx
            .send(Cmd::NatType(if mapped_ok {
                NatType::Cone
            } else {
                NatType::Unknown
            }))
            .await;
        tokio::time::sleep(Duration::from_secs(20)).await;
    }
}

fn local_ipv4s() -> Vec<Ipv4Addr> {
    let mut out = Vec::new();
    if let Ok(interfaces) = if_addrs::get_if_addrs() {
        for iface in interfaces {
            let if_addrs::IfAddr::V4(v4) = iface.addr else {
                continue;
            };
            let ip = v4.ip;
            if ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() {
                continue;
            }
            if !out.contains(&ip) {
                out.push(ip);
            }
        }
    }
    out
}

fn dedupe(candidates: &mut Vec<EndpointCandidate>) {
    let mut seen = HashSet::new();
    candidates.retain(|c| seen.insert(c.addr));
}

fn initial_snapshot(state: &NodeState, tun_name: &str, udp_port: u16) -> AgentSnapshot {
    AgentSnapshot {
        status: AgentStatus {
            version: env!("CARGO_PKG_VERSION").to_string(),
            node_id: Some(state.node_id),
            node_name: Some(state.node_name.clone()),
            overlay_ip: Some(state.overlay_ip),
            control_url: Some(state.control_url.clone()),
            control_connected: false,
            relay_connected: false,
            tun_name: Some(tun_name.to_string()),
            udp_port: Some(udp_port),
            nat_type: NatType::Unknown,
            peers_total: 0,
            peers_online: 0,
            uptime_secs: 0,
            map_version: 0,
            last_error: None,
        },
        peers: Vec::new(),
        shares: Vec::new(),
        peer_ips: BTreeMap::new(),
        self_name: state.node_name.clone(),
        self_ip: state.overlay_ip,
    }
}

#[allow(dead_code)]
fn _unused(_: SelfInfo, _: ControlEvent, _: PeerInfo) {}


#[cfg(test)]
mod tests {
    use super::*;
    use wm_proto::{AclAction, CompiledRule, PortRange};

    #[test]
    fn restrictive_map_invalidates_continuously_refreshed_tcp_udp_and_icmp() {
        for proto in [6, 17, 1] {
            let key = FlowKey {
                src: Ipv4Addr::new(100, 64, 0, 1),
                dst: Ipv4Addr::new(100, 64, 0, 2),
                sport: 12345, dport: 80, proto,
            };
            let mut flows = HashMap::new();
            record_flow(&mut flows, key);
            // Repeated responses must never replace the authorizing direction.
            for _ in 0..100 { record_flow(&mut flows, key.reverse()); }
            let allow_origin = CompiledAcl {
                version: 2, default_allow: false,
                rules: vec![CompiledRule {
                    action: AclAction::Accept,
                    src: "100.64.0.1/32".parse().unwrap(),
                    dst: "100.64.0.2/32".parse().unwrap(),
                    protocol: Some(proto),
                    ports: vec![PortRange { start: 80, end: 80 }],
                    comment: None,
                }],
            };
            revalidate_flows(&mut flows, &allow_origin);
            assert!(flows.contains_key(&key));
            assert!(flows.contains_key(&key.reverse()), "permitted responses survive");
            let deny = CompiledAcl { version: 3, default_allow: false, rules: vec![] };
            revalidate_flows(&mut flows, &deny);
            assert!(flows.is_empty(), "active flow must not bypass updated ACL");
            assert!(!deny.is_allowed(key.src, key.dst, proto, Some(key.dport)));
        }
    }

    #[test]
    fn reverse_only_permission_does_not_preserve_revoked_initiator() {
        let key = FlowKey {
            src: Ipv4Addr::new(100, 64, 0, 1), dst: Ipv4Addr::new(100, 64, 0, 2),
            sport: 12345, dport: 80, proto: 6,
        };
        let mut flows = HashMap::new();
        record_flow(&mut flows, key);
        record_flow(&mut flows, key.reverse());
        let acl = CompiledAcl {
            version: 2, default_allow: false,
            rules: vec![CompiledRule {
                action: AclAction::Accept,
                src: "100.64.0.2/32".parse().unwrap(),
                dst: "100.64.0.1/32".parse().unwrap(),
                protocol: None, ports: vec![], comment: None,
            }],
        };
        revalidate_flows(&mut flows, &acl);
        assert!(flows.is_empty());
    }
}
