//! STUN Binding client used to learn the public UDP mapping of the WireGuard
//! socket. Responses are routed here by the UDP receive task, so the mapping
//! always belongs to the same socket that carries data packets.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::sync::mpsc;

const MAGIC: u32 = 0x2112_A442;
const BINDING_REQUEST: u16 = 0x0001;
const BINDING_SUCCESS: u16 = 0x0101;
const ATTR_MAPPED: u16 = 0x0001;
const ATTR_XOR_MAPPED: u16 = 0x0020;

pub fn is_stun(buf: &[u8]) -> bool {
    buf.len() >= 20
        && u32::from_be_bytes(buf[4..8].try_into().unwrap()) == MAGIC
        && matches!(
            u16::from_be_bytes(buf[0..2].try_into().unwrap()),
            BINDING_REQUEST | BINDING_SUCCESS
        )
}

pub fn binding_request(txid: &[u8; 12]) -> Vec<u8> {
    let mut out = Vec::with_capacity(20);
    out.extend_from_slice(&BINDING_REQUEST.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&MAGIC.to_be_bytes());
    out.extend_from_slice(txid);
    out
}

pub fn parse_binding_response(buf: &[u8]) -> Option<SocketAddr> {
    if buf.len() < 20 {
        return None;
    }
    let typ = u16::from_be_bytes(buf[0..2].try_into().ok()?);
    if typ != BINDING_SUCCESS {
        return None;
    }
    let len = u16::from_be_bytes(buf[2..4].try_into().ok()?) as usize;
    if buf.len() < 20 + len {
        return None;
    }
    let magic = MAGIC.to_be_bytes();
    let mut pos = 20;
    let mut mapped = None;
    while pos + 4 <= 20 + len {
        let attr = u16::from_be_bytes(buf[pos..pos + 2].try_into().ok()?);
        let alen = u16::from_be_bytes(buf[pos + 2..pos + 4].try_into().ok()?) as usize;
        let body = buf.get(pos + 4..pos + 4 + alen)?;
        if attr == ATTR_XOR_MAPPED && alen >= 8 {
            let port = u16::from_be_bytes(body[2..4].try_into().ok()?)
                ^ ((MAGIC >> 16) as u16);
            let ip = Ipv4Addr::new(
                body[4] ^ magic[0],
                body[5] ^ magic[1],
                body[6] ^ magic[2],
                body[7] ^ magic[3],
            );
            mapped = Some(SocketAddr::new(IpAddr::V4(ip), port));
        } else if attr == ATTR_MAPPED && alen >= 8 {
            let port = u16::from_be_bytes(body[2..4].try_into().ok()?);
            let ip = Ipv4Addr::new(body[4], body[5], body[6], body[7]);
            mapped = Some(SocketAddr::new(IpAddr::V4(ip), port));
        }
        pos += 4 + alen.div_ceil(4) * 4;
    }
    mapped
}

pub async fn discover(
    socket: &UdpSocket,
    server: SocketAddr,
    rx: &mut mpsc::Receiver<(Vec<u8>, SocketAddr)>,
) -> anyhow::Result<SocketAddr> {
    let mut txid = [0u8; 12];
    getrandom::fill(&mut txid).map_err(|e| anyhow::anyhow!("getrandom: {e}"))?;
    let request = binding_request(&txid);
    socket.send_to(&request, server).await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            anyhow::bail!("STUN sem resposta de {server}");
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Some((buf, _from))) => {
                if buf.len() >= 20
                    && is_stun(&buf)
                    && u16::from_be_bytes([buf[0], buf[1]]) == BINDING_SUCCESS
                    && buf[8..20] == txid
                {
                    if let Some(addr) = parse_binding_response(&buf) {
                        return Ok(addr);
                    }
                }
            }
            Ok(None) => anyhow::bail!("canal STUN fechado"),
            Err(_) => anyhow::bail!("STUN sem resposta de {server}"),
        }
    }
}

pub async fn resolve_stun_url(raw: &str) -> anyhow::Result<SocketAddr> {
    let raw = raw.trim();
    let raw = raw
        .strip_prefix("stun:")
        .or_else(|| raw.strip_prefix("stuns:"))
        .unwrap_or(raw);
    let (host, port) = match raw.rsplit_once(':') {
        Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) => (
            host.trim_matches(['[', ']']),
            port.parse::<u16>()?,
        ),
        _ => (raw, 3478),
    };
    let mut addrs = tokio::net::lookup_host((host, port)).await?;
    addrs
        .next()
        .ok_or_else(|| anyhow::anyhow!("host STUN sem endereço: {host}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_is_stun() {
        let req = binding_request(&[1; 12]);
        assert!(is_stun(&req));
        assert_eq!(parse_binding_response(&req), None);
    }
}
