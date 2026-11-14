//! Userspace WireGuard peer engine on top of boringtun.
//!
//! One `Tunn` exists per peer. The index passed to `Tunn::new` is unique per
//! peer and boringtun stores it as `index << 8`, so a data packet's receiver
//! index maps back to the peer with `receiver_idx >> 8` in O(1). Handshake
//! initiations have no receiver index; they are rare and we try the cached
//! endpoint peer first.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use boringtun::noise::{Packet, Tunn, TunnResult};
use boringtun::x25519::{PublicKey, StaticSecret};

use wm_proto::{EndpointCandidate, NodeId, PathKind, PeerInfo, PeerStats};

#[derive(Debug, Clone)]
pub enum WgOut {
    Network(Vec<u8>),
    Tunnel(Vec<u8>),
    Done,
    Err(String),
}

pub struct WgPeer {
    pub id: NodeId,
    pub name: String,
    pub display_name: String,
    pub about: String,
    pub overlay_ip: Ipv4Addr,
    pub public_key: String,
    pub endpoints: Vec<SocketAddr>,
    pub endpoint: Option<SocketAddr>,
    pub path: PathKind,
    pub relay_online: bool,
    pub last_initiate: Option<Instant>,
    pub last_direct_attempt: Option<Instant>,
    pub last_error: Option<String>,
    tunn: Tunn,
    slot: u32,
}

pub struct WgEngine {
    local_secret: StaticSecret,
    local_public: PublicKey,
    peers: HashMap<NodeId, WgPeer>,
    slots: HashMap<u32, NodeId>,
    next_slot: u32,
}

impl WgEngine {
    pub fn new(local_secret: StaticSecret) -> Self {
        let local_public = PublicKey::from(&local_secret);
        Self {
            local_secret,
            local_public,
            peers: HashMap::new(),
            slots: HashMap::new(),
            next_slot: 1,
        }
    }

    pub fn local_public(&self) -> String {
        STANDARD.encode(self.local_public.as_bytes())
    }

    pub fn peers(&self) -> impl Iterator<Item = &WgPeer> {
        self.peers.values()
    }

    pub fn peer(&self, id: NodeId) -> Option<&WgPeer> {
        self.peers.get(&id)
    }

    pub fn peer_mut(&mut self, id: NodeId) -> Option<&mut WgPeer> {
        self.peers.get_mut(&id)
    }

    pub fn peer_by_name(&self, name: &str) -> Option<&WgPeer> {
        self.peers.values().find(|p| p.name == name)
    }

    pub fn set_path(&mut self, id: NodeId, path: PathKind) {
        if let Some(p) = self.peers.get_mut(&id) {
            p.path = path;
        }
    }

    pub fn set_endpoint(&mut self, id: NodeId, endpoint: SocketAddr) {
        if let Some(p) = self.peers.get_mut(&id) {
            p.endpoint = Some(endpoint);
            p.path = PathKind::Direct;
        }
    }

    pub fn reset_path(&mut self, id: NodeId) {
        if let Some(p) = self.peers.get_mut(&id) {
            p.endpoint = None;
            p.path = PathKind::None;
            p.last_initiate = None;
            p.last_direct_attempt = None;
        }
    }

    pub fn set_relay_online(&mut self, id: NodeId, online: bool) {
        if let Some(p) = self.peers.get_mut(&id) {
            p.relay_online = online;
            if !online && p.path == PathKind::Relay {
                p.path = PathKind::None;
            }
        }
    }

    /// Reconciles the peer set from a map snapshot. Existing sessions and
    /// endpoints survive; only metadata/candidates are refreshed.
    pub fn apply_peers(&mut self, peers: &[PeerInfo]) {
        let wanted: HashSet<NodeId> = peers.iter().map(|p| p.id).collect();
        let removed: Vec<NodeId> = self
            .peers
            .keys()
            .copied()
            .filter(|id| !wanted.contains(id))
            .collect();
        for id in removed {
            if let Some(peer) = self.peers.remove(&id) {
                self.slots.remove(&peer.slot);
            }
        }

        for info in peers {
            if let Some(peer) = self.peers.get_mut(&info.id) {
                peer.name = info.name.clone();
                peer.display_name = info.display_name.clone();
                peer.about = info.about.clone();
                peer.overlay_ip = info.overlay_ip;
                peer.public_key = info.wg_public_key.clone();
                peer.endpoints = endpoint_order(&info.endpoints);
                continue;
            }
            let Ok(pub_bytes) = STANDARD.decode(&info.wg_public_key) else {
                tracing::warn!(peer = %info.name, "chave pública WireGuard inválida; ignorando peer");
                continue;
            };
            let Ok(arr) = <[u8; 32]>::try_from(pub_bytes.as_slice()) else {
                tracing::warn!(peer = %info.name, "chave pública WireGuard não tem 32 bytes");
                continue;
            };
            let public = PublicKey::from(arr);
            let Some(slot) = self.alloc_slot() else {
                tracing::error!("sem slots de WireGuard disponíveis");
                break;
            };
            let tunn = Tunn::new(
                self.local_secret.clone(),
                public,
                None,
                Some(25),
                slot,
                None,
            );
            self.slots.insert(slot, info.id);
            self.peers.insert(
                info.id,
                WgPeer {
                    id: info.id,
                    name: info.name.clone(),
                    display_name: info.display_name.clone(),
                    about: info.about.clone(),
                    overlay_ip: info.overlay_ip,
                    public_key: info.wg_public_key.clone(),
                    endpoints: endpoint_order(&info.endpoints),
                    endpoint: None,
                    path: PathKind::None,
                    relay_online: info.online,
                    last_initiate: None,
                    last_direct_attempt: None,
                    last_error: None,
                    tunn,
                    slot,
                },
            );
        }
    }

    fn alloc_slot(&mut self) -> Option<u32> {
        for _ in 0..0x00ff_ffff {
            let slot = self.next_slot;
            self.next_slot = if self.next_slot >= 0x00ff_ffff {
                1
            } else {
                self.next_slot + 1
            };
            if slot != 0 && !self.slots.contains_key(&slot) {
                return Some(slot);
            }
        }
        None
    }

    /// Feeds one UDP datagram and returns the peer that accepted it plus the
    /// result. `preferred` is the endpoint-cache guess for handshake inits.
    pub fn receive(
        &mut self,
        datagram: &[u8],
        src_ip: Option<IpAddr>,
        preferred: Option<NodeId>,
    ) -> Vec<(NodeId, WgOut)> {
        let receiver = match Tunn::parse_incoming_packet(datagram) {
            Ok(Packet::PacketData(p)) => Some(p.receiver_idx),
            Ok(Packet::HandshakeResponse(p)) => Some(p.receiver_idx),
            Ok(Packet::PacketCookieReply(p)) => Some(p.receiver_idx),
            Ok(Packet::HandshakeInit(_)) => None,
            Err(_) => return Vec::new(),
        };

        if let Some(receiver) = receiver {
            let slot = receiver >> 8;
            let Some(id) = self.slots.get(&slot).copied() else {
                return Vec::new();
            };
            let out = self.decapsulate(id, src_ip, datagram);
            return vec![(id, out)];
        }

        // Handshake init: try the endpoint cache first, then every peer.
        let mut order = Vec::with_capacity(self.peers.len() + 1);
        if let Some(pref) = preferred {
            if self.peers.contains_key(&pref) {
                order.push(pref);
            }
        }
        for id in self.peers.keys() {
            if !order.contains(id) {
                order.push(*id);
            }
        }
        for id in order {
            match self.decapsulate(id, src_ip, datagram) {
                WgOut::Err(_) | WgOut::Done => continue,
                out => return vec![(id, out)],
            }
        }
        Vec::new()
    }

    pub fn decapsulate(
        &mut self,
        id: NodeId,
        src_ip: Option<IpAddr>,
        datagram: &[u8],
    ) -> WgOut {
        let Some(peer) = self.peers.get_mut(&id) else {
            return WgOut::Err("peer desconhecido".into());
        };
        let mut dst = vec![0u8; 2048];
        let out = match peer.tunn.decapsulate(src_ip, datagram, &mut dst) {
            TunnResult::Done => WgOut::Done,
            TunnResult::Err(err) => {
                let msg = format!("{err:?}");
                peer.last_error = Some(msg.clone());
                WgOut::Err(msg)
            }
            TunnResult::WriteToNetwork(buf) => {
                let data = buf.to_vec();
                peer.last_error = None;
                WgOut::Network(data)
            }
            TunnResult::WriteToTunnelV4(buf, _) => {
                let data = buf.to_vec();
                peer.last_error = None;
                WgOut::Tunnel(data)
            }
            TunnResult::WriteToTunnelV6(_, _) => WgOut::Err("IPv6 no overlay não suportado".into()),
        };
        out
    }

    pub fn encapsulate(&mut self, id: NodeId, packet: &[u8]) -> WgOut {
        let Some(peer) = self.peers.get_mut(&id) else {
            return WgOut::Err("peer desconhecido".into());
        };
        let mut dst = vec![0u8; packet.len() + 256];
        match peer.tunn.encapsulate(packet, &mut dst) {
            TunnResult::Done => WgOut::Done,
            TunnResult::Err(err) => {
                let msg = format!("{err:?}");
                peer.last_error = Some(msg.clone());
                WgOut::Err(msg)
            }
            TunnResult::WriteToNetwork(buf) => {
                let data = buf.to_vec();
                WgOut::Network(data)
            }
            TunnResult::WriteToTunnelV4(buf, _) => WgOut::Tunnel(buf.to_vec()),
            TunnResult::WriteToTunnelV6(_, _) => WgOut::Err("IPv6 não suportado".into()),
        }
    }

    /// Forces a handshake initiation (used for lazy peers and diagnostics).
    pub fn initiate(&mut self, id: NodeId) -> WgOut {
        let Some(peer) = self.peers.get_mut(&id) else {
            return WgOut::Err("peer desconhecido".into());
        };
        let now = Instant::now();
        if let Some(last) = peer.last_initiate {
            if now.duration_since(last) < Duration::from_secs(1) {
                return WgOut::Done;
            }
        }
        peer.last_initiate = Some(now);
        let mut dst = vec![0u8; 256];
        match peer.tunn.format_handshake_initiation(&mut dst, false) {
            TunnResult::WriteToNetwork(buf) => WgOut::Network(buf.to_vec()),
            TunnResult::Done => WgOut::Done,
            TunnResult::Err(err) => WgOut::Err(format!("{err:?}")),
            TunnResult::WriteToTunnelV4(_, _) | TunnResult::WriteToTunnelV6(_, _) => WgOut::Done,
        }
    }

    pub fn update_timers(&mut self, id: NodeId) -> WgOut {
        let Some(peer) = self.peers.get_mut(&id) else {
            return WgOut::Done;
        };
        let mut dst = vec![0u8; 2048];
        match peer.tunn.update_timers(&mut dst) {
            TunnResult::Done => WgOut::Done,
            TunnResult::Err(err) => WgOut::Err(format!("{err:?}")),
            TunnResult::WriteToNetwork(buf) => WgOut::Network(buf.to_vec()),
            TunnResult::WriteToTunnelV4(buf, _) => WgOut::Tunnel(buf.to_vec()),
            TunnResult::WriteToTunnelV6(_, _) => WgOut::Done,
        }
    }

    /// Drains packets that boringtun queued while waiting for a handshake.
    /// Call this after a handshake completes.
    pub fn drain(&mut self, id: NodeId) -> Vec<WgOut> {
        let mut out = Vec::new();
        for _ in 0..64 {
            let Some(peer) = self.peers.get_mut(&id) else {
                break;
            };
            let mut dst = vec![0u8; 2048];
            match peer.tunn.decapsulate(None, &[], &mut dst) {
                TunnResult::Done => break,
                TunnResult::Err(err) => {
                    peer.last_error = Some(format!("{err:?}"));
                    break;
                }
                TunnResult::WriteToNetwork(buf) => out.push(WgOut::Network(buf.to_vec())),
                TunnResult::WriteToTunnelV4(buf, _) => out.push(WgOut::Tunnel(buf.to_vec())),
                TunnResult::WriteToTunnelV6(_, _) => break,
            }
        }
        out
    }

    pub fn handshake_age(&self, id: NodeId) -> Option<Duration> {
        self.peers
            .get(&id)
            .and_then(|p| p.tunn.time_since_last_handshake())
    }

    pub fn handshake_expired(&self, id: NodeId) -> bool {
        self.peers
            .get(&id)
            .map(|p| p.tunn.is_expired())
            .unwrap_or(true)
    }

    pub fn stats(&self, id: NodeId) -> Option<(Option<Duration>, u64, u64, Option<u32>)> {
        self.peers.get(&id).map(|p| {
            let (age, tx, rx, _loss, rtt) = p.tunn.stats();
            (age, tx as u64, rx as u64, rtt)
        })
    }

    pub fn peer_stats(&self) -> Vec<PeerStats> {
        self.peers
            .values()
            .map(|p| {
                let (age, tx, rx, _loss, rtt) = p.tunn.stats();
                PeerStats {
                    peer_id: p.id,
                    path: p.path,
                    rtt_ms: rtt,
                    last_handshake_secs: age.map(|d| d.as_secs()),
                    rx_bytes: rx as u64,
                    tx_bytes: tx as u64,
                    relay_online: p.relay_online,
                    endpoint: p.endpoint.map(|e| e.to_string()),
                    last_error: p.last_error.clone(),
                }
            })
            .collect()
    }
}

fn endpoint_order(candidates: &[EndpointCandidate]) -> Vec<SocketAddr> {
    let mut sorted = candidates.to_vec();
    sorted.sort_by_key(|c| match c.kind {
        wm_proto::EndpointKind::Stun => 0,
        wm_proto::EndpointKind::Lan => 1,
        wm_proto::EndpointKind::Previous => 2,
        wm_proto::EndpointKind::Manual => 3,
    });
    let mut out = Vec::new();
    for c in sorted {
        if c.addr.is_ipv4() && !out.contains(&c.addr) {
            out.push(c.addr);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;
    use wm_proto::PeerInfo;

    fn peer_info(id: Uuid, name: &str, ip: &str, public: &str) -> PeerInfo {
        PeerInfo {
            id,
            name: name.into(),
            hostname: format!("{name}.local"),
            overlay_ip: ip.parse().unwrap(),
            wg_public_key: public.into(),
            tags: vec![],
            labels: Default::default(),
            endpoints: vec![],
            online: true,
            last_seen: None,
            display_name: String::new(),
            about: String::new(),
        }
    }

    #[test]
    fn two_peers_handshake_and_data() {
        let (a_secret, a_public, _, a_public_b64) = super::super::state::generate_keypair();
        let (b_secret, b_public, _, b_public_b64) = super::super::state::generate_keypair();
        let a_id = Uuid::new_v4();
        let b_id = Uuid::new_v4();
        let mut a = WgEngine::new(a_secret);
        let mut b = WgEngine::new(b_secret);
        a.apply_peers(&[peer_info(b_id, "b", "10.66.0.3", &b_public_b64)]);
        b.apply_peers(&[peer_info(a_id, "a", "10.66.0.2", &a_public_b64)]);
        let _ = (a_public, b_public);

        // A initiates.
        let init = a.initiate(b_id);
        let WgOut::Network(init) = init else {
            panic!("sem handshake init");
        };
        let outs = b.receive(&init, Some("10.66.0.2".parse().unwrap()), None);
        let (from_a, WgOut::Network(response)) = outs.into_iter().next().unwrap() else {
            panic!("sem resposta");
        };
        assert_eq!(from_a, a_id);
        let outs = a.receive(&response, Some("10.66.0.3".parse().unwrap()), None);
        let keepalive = match outs.into_iter().next().unwrap().1 {
            WgOut::Network(data) => data,
            other => panic!("esperava keepalive, veio {other:?}"),
        };
        let _ = b.receive(&keepalive, Some("10.66.0.2".parse().unwrap()), None);

        // Flush any queued handshake packets.
        for _ in 0..4 {
            let _ = a.drain(b_id);
            let _ = b.drain(a_id);
        }

        // Must be at least an IPv4 header + WG tag (boringtun's parser rejects tiny frames).
        let mut payload = vec![0u8; 40];
        payload[0] = 0x45;
        payload[2..4].copy_from_slice(&40u16.to_be_bytes());
        let enc = a.encapsulate(b_id, &payload);
        let WgOut::Network(wire) = enc else {
            panic!("sem dados cifrados");
        };
        let outs = b.receive(&wire, Some("10.66.0.2".parse().unwrap()), None);
        let (from_a, WgOut::Tunnel(plain)) = outs.into_iter().next().unwrap() else {
            panic!("sem plaintext");
        };
        assert_eq!(from_a, a_id);
        assert_eq!(plain, payload);
    }
}
