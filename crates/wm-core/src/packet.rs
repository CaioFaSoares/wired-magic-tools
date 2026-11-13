//! Small IPv4 packet parser for routing and ACL enforcement.

use std::net::Ipv4Addr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketMeta {
    pub src: Ipv4Addr,
    pub dst: Ipv4Addr,
    pub protocol: u8,
    pub src_port: Option<u16>,
    pub dst_port: Option<u16>,
    pub icmp_type: Option<u8>,
    pub icmp_code: Option<u8>,
}

impl PacketMeta {
    pub fn is_icmp_echo_reply(&self) -> bool {
        self.protocol == 1 && self.icmp_type == Some(0)
    }
    pub fn is_icmp_echo_request(&self) -> bool {
        self.protocol == 1 && self.icmp_type == Some(8)
    }
}

pub fn parse(pkt: &[u8]) -> Option<PacketMeta> {
    if pkt.len() < 20 || pkt[0] >> 4 != 4 {
        return None;
    }
    let ihl = usize::from(pkt[0] & 0x0f) * 4;
    if ihl < 20 || pkt.len() < ihl {
        return None;
    }
    let total_len = usize::from(u16::from_be_bytes([pkt[2], pkt[3]]));
    if total_len < ihl || total_len > pkt.len() {
        return None;
    }
    let protocol = pkt[9];
    let src = Ipv4Addr::new(pkt[12], pkt[13], pkt[14], pkt[15]);
    let dst = Ipv4Addr::new(pkt[16], pkt[17], pkt[18], pkt[19]);
    let l4 = &pkt[ihl..total_len];
    let (src_port, dst_port) = match protocol {
        6 | 17 if l4.len() >= 4 => (
            Some(u16::from_be_bytes([l4[0], l4[1]])),
            Some(u16::from_be_bytes([l4[2], l4[3]])),
        ),
        _ => (None, None),
    };
    let (icmp_type, icmp_code) = if protocol == 1 && l4.len() >= 2 {
        (Some(l4[0]), Some(l4[1]))
    } else {
        (None, None)
    };
    Some(PacketMeta {
        src,
        dst,
        protocol,
        src_port,
        dst_port,
        icmp_type,
        icmp_code,
    })
}

pub fn dst_ip(pkt: &[u8]) -> Option<Ipv4Addr> {
    parse(pkt).map(|m| m.dst)
}

/// Returns `(id, seq)` for an ICMPv4 echo packet.
pub fn icmp_id_seq(pkt: &[u8]) -> Option<(u16, u16)> {
    let meta = parse(pkt)?;
    if meta.protocol != 1 {
        return None;
    }
    let ihl = usize::from(pkt[0] & 0x0f) * 4;
    if pkt.len() < ihl + 8 {
        return None;
    }
    let id = u16::from_be_bytes([pkt[ihl + 4], pkt[ihl + 5]]);
    let seq = u16::from_be_bytes([pkt[ihl + 6], pkt[ihl + 7]]);
    Some((id, seq))
}

/// Builds an ICMPv4 echo request from `src` to `dst`.
pub fn icmp_echo_request(src: Ipv4Addr, dst: Ipv4Addr, id: u16, seq: u16) -> Vec<u8> {
    let mut pkt = vec![0u8; 20 + 8 + 16];
    pkt[0] = 0x45;
    pkt[1] = 0;
    let total = pkt.len() as u16;
    pkt[2..4].copy_from_slice(&total.to_be_bytes());
    pkt[8] = 64; // TTL
    pkt[9] = 1; // ICMP
    pkt[12..16].copy_from_slice(&src.octets());
    pkt[16..20].copy_from_slice(&dst.octets());
    let csum = ip_checksum(&pkt[..20]);
    pkt[10..12].copy_from_slice(&csum.to_be_bytes());

    pkt[20] = 8; // echo request
    pkt[21] = 0;
    pkt[24..26].copy_from_slice(&id.to_be_bytes());
    pkt[26..28].copy_from_slice(&seq.to_be_bytes());
    // 16 bytes of payload with a timestamp-ish pattern.
    for (i, b) in pkt[28..].iter_mut().enumerate() {
        *b = i as u8;
    }
    let csum = ip_checksum(&pkt[20..]);
    pkt[22..24].copy_from_slice(&csum.to_be_bytes());
    pkt
}

fn ip_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut chunks = data.chunks_exact(2);
    for chunk in &mut chunks {
        sum += u32::from(u16::from_be_bytes([chunk[0], chunk[1]]));
    }
    if let Some(&last) = chunks.remainder().first() {
        sum += u32::from(last) << 8;
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_udp_packet() {
        let mut pkt = vec![0u8; 28];
        pkt[0] = 0x45;
        pkt[2..4].copy_from_slice(&28u16.to_be_bytes());
        pkt[9] = 17;
        pkt[12..16].copy_from_slice(&[10, 66, 0, 2]);
        pkt[16..20].copy_from_slice(&[10, 66, 0, 3]);
        pkt[20..22].copy_from_slice(&1234u16.to_be_bytes());
        pkt[22..24].copy_from_slice(&8421u16.to_be_bytes());
        let meta = parse(&pkt).unwrap();
        assert_eq!(meta.dst, Ipv4Addr::new(10, 66, 0, 3));
        assert_eq!(meta.dst_port, Some(8421));
        assert_eq!(meta.protocol, 17);
    }

    #[test]
    fn reads_icmp_echo_id_seq() {
        let pkt = icmp_echo_request(
            Ipv4Addr::new(10, 66, 0, 2),
            Ipv4Addr::new(10, 66, 0, 3),
            0x1234,
            9,
        );
        assert_eq!(icmp_id_seq(&pkt), Some((0x1234, 9)));
    }

    #[test]
    fn builds_valid_icmp_request() {
        let pkt = icmp_echo_request(
            Ipv4Addr::new(10, 66, 0, 2),
            Ipv4Addr::new(10, 66, 0, 3),
            7,
            1,
        );
        assert_eq!(pkt.len(), 44);
        let meta = parse(&pkt).unwrap();
        assert!(meta.is_icmp_echo_request());
        assert_eq!(ip_checksum(&pkt[..20]), 0);
        assert_eq!(ip_checksum(&pkt[20..]), 0);
    }
}
