//! Minimal RFC 5389 STUN Binding server and client helpers.
//!
//! Only Binding requests/responses are implemented, which is all the mesh needs:
//! agents use the server-reflexive address to publish their UDP mapping. We use
//! the same socket that carries WireGuard packets, so the STUN mapping is the
//! mapping peers have to punch to.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use tokio::net::UdpSocket;

const MAGIC: u32 = 0x2112_A442;
const BINDING_REQUEST: u16 = 0x0001;
const BINDING_SUCCESS: u16 = 0x0101;
const ATTR_MAPPED: u16 = 0x0001;
const ATTR_XOR_MAPPED: u16 = 0x0020;
const ATTR_SOFTWARE: u16 = 0x8022;

pub fn is_stun(buf: &[u8]) -> bool {
    buf.len() >= 20
        && u32::from_be_bytes(buf[4..8].try_into().unwrap()) == MAGIC
        && matches!(
            u16::from_be_bytes(buf[0..2].try_into().unwrap()),
            BINDING_REQUEST | BINDING_SUCCESS
        )
}

/// Builds a Binding request. The caller keeps the transaction id to match the
/// response.
pub fn binding_request(txid: &[u8; 12]) -> Vec<u8> {
    let mut out = Vec::with_capacity(20);
    out.extend_from_slice(&BINDING_REQUEST.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&MAGIC.to_be_bytes());
    out.extend_from_slice(txid);
    out
}

/// Parses a Binding success response and returns the observed address.
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
    let mut pos = 20;
    let magic = MAGIC.to_be_bytes();
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

/// Builds a Binding success response for `source`.
pub fn binding_response(source: SocketAddr, txid: &[u8; 12]) -> Vec<u8> {
    let mut attrs = Vec::new();
    let SocketAddr::V4(v4) = source else {
        return Vec::new();
    };
    attrs.extend_from_slice(&ATTR_XOR_MAPPED.to_be_bytes());
    attrs.extend_from_slice(&8u16.to_be_bytes());
    attrs.push(0);
    attrs.push(0x01); // IPv4
    let port = v4.port() ^ ((MAGIC >> 16) as u16);
    attrs.extend_from_slice(&port.to_be_bytes());
    let ip = v4.ip().octets();
    let magic = MAGIC.to_be_bytes();
    let xored = [
        ip[0] ^ magic[0],
        ip[1] ^ magic[1],
        ip[2] ^ magic[2],
        ip[3] ^ magic[3],
    ];
    attrs.extend_from_slice(&xored);
    attrs.extend_from_slice(&ATTR_SOFTWARE.to_be_bytes());
    let software = b"wired-magic";
    attrs.extend_from_slice(&(software.len() as u16).to_be_bytes());
    attrs.extend_from_slice(software);
    while attrs.len() % 4 != 0 {
        attrs.push(0);
    }

    let mut out = Vec::with_capacity(20 + attrs.len());
    out.extend_from_slice(&BINDING_SUCCESS.to_be_bytes());
    out.extend_from_slice(&(attrs.len() as u16).to_be_bytes());
    out.extend_from_slice(&MAGIC.to_be_bytes());
    out.extend_from_slice(txid);
    out.extend_from_slice(&attrs);
    out
}

/// Runs a STUN Binding server forever. The socket also serves as a discovery
/// target; only Binding requests are answered.
pub async fn run_stun_server(listen: SocketAddr) -> anyhow::Result<()> {
    let socket = UdpSocket::bind(listen).await?;
    tracing::info!(%listen, "STUN ouvindo");
    let mut buf = [0u8; 1500];
    loop {
        let (n, from) = match socket.recv_from(&mut buf).await {
            Ok(v) => v,
            Err(err) => {
                tracing::warn!(%err, "STUN recv");
                continue;
            }
        };
        if n < 20 || !is_stun(&buf[..n]) {
            continue;
        }
        let typ = u16::from_be_bytes([buf[0], buf[1]]);
        if typ != BINDING_REQUEST {
            continue;
        }
        let mut txid = [0u8; 12];
        txid.copy_from_slice(&buf[8..20]);
        let response = binding_response(from, &txid);
        if !response.is_empty() {
            let _ = socket.send_to(&response, from).await;
        }
    }
}

/// Sends a Binding request over an existing socket and waits for the matching
/// response.
pub async fn discover_mapped(
    socket: &UdpSocket,
    server: SocketAddr,
) -> anyhow::Result<SocketAddr> {
    use tokio::time::{Duration, timeout};
    let mut txid = [0u8; 12];
    getrandom::fill(&mut txid)?;
    let request = binding_request(&txid);
    socket.send_to(&request, server).await?;
    let mut buf = [0u8; 1500];
    let deadline = Duration::from_secs(3);
    loop {
        let (n, _from) = timeout(deadline, socket.recv_from(&mut buf)).await??;
        if n >= 20 && is_stun(&buf[..n]) && buf[8..20] == txid {
            if let Some(addr) = parse_binding_response(&buf[..n]) {
                return Ok(addr);
            }
        }
    }
}

pub fn parse_server_url(raw: &str) -> Option<(String, u16)> {
    let raw = raw.trim();
    let raw = raw.strip_prefix("stun:").or_else(|| raw.strip_prefix("stuns:")).unwrap_or(raw);
    if raw.is_empty() {
        return None;
    }
    if let Some((host, port)) = raw.rsplit_once(':') {
        if let Ok(port) = port.parse::<u16>() {
            return Some((host.trim_matches(['[', ']']).to_string(), port));
        }
    }
    Some((raw.to_string(), 3478))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_response() {
        let txid = [7u8; 12];
        let source: SocketAddr = "203.0.113.9:51820".parse().unwrap();
        let response = binding_response(source, &txid);
        assert!(is_stun(&response));
        assert_eq!(parse_binding_response(&response), Some(source));
    }

    #[test]
    fn parses_urls() {
        assert_eq!(
            parse_server_url("stun:mesh.example.com:3478"),
            Some(("mesh.example.com".into(), 3478))
        );
        assert_eq!(
            parse_server_url("mesh.example.com"),
            Some(("mesh.example.com".into(), 3478))
        );
    }
}
