//! ACL compilation and simulation.
//!
//! A policy is authored with human selectors (`*`, `name:mac-01`, `tag:mac`,
//! `10.66.0.0/24`) and compiled to concrete IPv4 rules. The same compiled rules
//! are shipped in the map and enforced by every agent on egress and ingress.

use std::net::Ipv4Addr;

use ipnet::Ipv4Net;

use wm_proto::{
    AclAction, AclPolicy, AclView, CompiledAcl, CompiledRule, PortRange, SimulateRequest,
    SimulateResponse,
};

use crate::db::NodeRow;

#[derive(Debug, Clone)]
pub struct CompileOutcome {
    pub compiled: CompiledAcl,
    pub warnings: Vec<String>,
}

pub fn compile_policy(
    policy: &AclPolicy,
    nodes: &[NodeRow],
    overlay: Ipv4Net,
) -> Result<CompileOutcome, String> {
    let mut warnings = Vec::new();
    let mut rules = Vec::new();
    for (idx, rule) in policy.rules.iter().enumerate() {
        let srcs = expand_selectors(&rule.src, nodes, overlay, &mut warnings, idx, "src")?;
        let dsts = expand_selectors(&rule.dst, nodes, overlay, &mut warnings, idx, "dst")?;
        let protocol = parse_protocol(rule.protocol.as_deref())?;
        let ports = parse_ports(&rule.ports)?;
        for src in &srcs {
            for dst in &dsts {
                rules.push(CompiledRule {
                    action: rule.action,
                    src: *src,
                    dst: *dst,
                    protocol,
                    ports: ports.clone(),
                    comment: rule.comment.clone(),
                });
                if rules.len() > 8192 {
                    return Err("política expandiu para mais de 8192 regras; use tags/CIDRs".into());
                }
            }
        }
    }
    // An unresolved selector must never turn a restrictive policy into allow-all.
    let default_allow = policy.rules.is_empty();
    Ok(CompileOutcome {
        compiled: CompiledAcl {
            version: policy.version,
            default_allow,
            rules,
        },
        warnings,
    })
}

fn expand_selectors(
    selectors: &[String],
    nodes: &[NodeRow],
    overlay: Ipv4Net,
    warnings: &mut Vec<String>,
    rule_index: usize,
    side: &str,
) -> Result<Vec<Ipv4Net>, String> {
    if selectors.is_empty() {
        return Err(format!("regra {}: {side} vazio", rule_index + 1));
    }
    let mut out = Vec::new();
    for raw in selectors {
        let sel = raw.trim();
        if sel.is_empty() || sel == "*" || sel.eq_ignore_ascii_case("any") {
            out.push(overlay);
            continue;
        }
        if let Some(name) = sel.strip_prefix("name:") {
            match nodes.iter().find(|n| n.name == name.trim()) {
                Some(node) => out.push(host_net(node.overlay_ip())),
                None => warnings.push(format!(
                    "regra {}: nome '{name}' não existe; seletor ignorado",
                    rule_index + 1
                )),
            }
            continue;
        }
        if let Some(tag) = sel.strip_prefix("tag:") {
            let tag = tag.trim();
            let mut found = false;
            for node in nodes {
                if node.tags().iter().any(|t| t == tag) {
                    out.push(host_net(node.overlay_ip()));
                    found = true;
                }
            }
            if !found {
                warnings.push(format!(
                    "regra {}: nenhum nó com a tag '{tag}'; seletor ignorado",
                    rule_index + 1
                ));
            }
            continue;
        }
        if sel.contains('/') || sel.parse::<Ipv4Addr>().is_ok() {
            let net = if sel.contains('/') {
                sel.parse::<Ipv4Net>()
                    .map_err(|_| format!("regra {}: CIDR inválido '{sel}'", rule_index + 1))?
            } else {
                host_net(
                    sel.parse::<Ipv4Addr>()
                        .map_err(|_| format!("regra {}: IP inválido '{sel}'", rule_index + 1))?,
                )
            };
            out.push(net);
            continue;
        }
        return Err(format!(
            "regra {}: seletor '{sel}' inválido (use *, name:, tag: ou CIDR)",
            rule_index + 1
        ));
    }
    if out.is_empty() {
        // Every selector was unknown; an empty side would be dangerous to
        // silently accept, so make the rule inert instead of broad.
        warnings.push(format!(
            "regra {}: nenhum seletor resolvido em {side}; regra ignorada",
            rule_index + 1
        ));
    }
    Ok(out)
}

fn host_net(ip: Ipv4Addr) -> Ipv4Net {
    Ipv4Net::new(ip, 32).expect("/32")
}

fn parse_protocol(raw: Option<&str>) -> Result<Option<u8>, String> {
    let Some(raw) = raw else { return Ok(None) };
    match raw.trim().to_ascii_lowercase().as_str() {
        "" | "*" | "any" => Ok(None),
        "tcp" => Ok(Some(6)),
        "udp" => Ok(Some(17)),
        "icmp" => Ok(Some(1)),
        other => match other.parse::<u8>() {
            Ok(n) => Ok(Some(n)),
            Err(_) => Err(format!("protocolo inválido '{other}'")),
        },
    }
}

fn parse_ports(ports: &[String]) -> Result<Vec<PortRange>, String> {
    let mut out = Vec::new();
    for raw in ports {
        let raw = raw.trim();
        if raw.is_empty() || raw == "*" {
            continue;
        }
        if let Some((a, b)) = raw.split_once('-') {
            let start: u16 = a
                .trim()
                .parse()
                .map_err(|_| format!("porta inválida '{raw}'"))?;
            let end: u16 = b
                .trim()
                .parse()
                .map_err(|_| format!("porta inválida '{raw}'"))?;
            if start > end {
                return Err(format!("faixa de portas invertida '{raw}'"));
            }
            out.push(PortRange { start, end });
        } else {
            let p: u16 = raw
                .parse()
                .map_err(|_| format!("porta inválida '{raw}'"))?;
            out.push(PortRange::single(p));
        }
    }
    Ok(out)
}

pub fn validate_yaml(
    yaml: &str,
    nodes: &[NodeRow],
    overlay: Ipv4Net,
) -> Result<(AclPolicy, CompileOutcome), String> {
    let policy: AclPolicy = serde_yaml::from_str(yaml).map_err(|e| format!("YAML inválido: {e}"))?;
    let outcome = compile_policy(&policy, nodes, overlay)?;
    Ok((policy, outcome))
}

pub fn view_for(
    policy: &AclPolicy,
    compiled: CompiledAcl,
    updated_at: Option<chrono::DateTime<chrono::Utc>>,
) -> AclView {
    AclView {
        policy: policy.clone(),
        compiled,
        updated_at,
    }
}

pub fn simulate(
    compiled: &CompiledAcl,
    nodes: &[NodeRow],
    req: &SimulateRequest,
) -> Result<SimulateResponse, String> {
    let src = resolve_one(&req.src, nodes).ok_or_else(|| format!("origem '{}' não encontrada", req.src))?;
    let dst = resolve_one(&req.dst, nodes).ok_or_else(|| format!("destino '{}' não encontrado", req.dst))?;
    let protocol = parse_protocol(req.protocol.as_deref())?.unwrap_or(6);
    let port = req.port;
    if matches!(protocol, 6 | 17) && port.is_none() {
        return Err("informe a porta para tcp/udp".into());
    }
    for (i, rule) in compiled.rules.iter().enumerate() {
        if rule.matches(src, dst, protocol, port) {
            return Ok(SimulateResponse {
                allowed: rule.action == AclAction::Accept,
                rule_index: Some(i),
                reason: format!(
                    "regra {} ({:?}, protocolo {:?}, portas {:?})",
                    i + 1,
                    rule.action,
                    rule.protocol,
                    rule.ports
                ),
            });
        }
    }
    Ok(SimulateResponse {
        allowed: compiled.default_allow,
        rule_index: None,
        reason: if compiled.default_allow {
            "nenhuma regra casou; política aberta (default allow)".into()
        } else {
            "nenhuma regra casou; deny implícito".into()
        },
    })
}

fn resolve_one(value: &str, nodes: &[NodeRow]) -> Option<Ipv4Addr> {
    let value = value.trim();
    if let Ok(ip) = value.parse::<Ipv4Addr>() {
        return Some(ip);
    }
    if value.contains('/') {
        if let Ok(net) = value.parse::<Ipv4Net>() {
            return Some(net.network());
        }
    }
    nodes
        .iter()
        .find(|n| n.name == value)
        .map(|n| n.overlay_ip())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::NodeRow;
    use wm_proto::AclRule;

    fn node(id: &str, name: &str, ip: &str, tags: &str) -> NodeRow {
        NodeRow {
            id: id.into(),
            name: name.into(),
            hostname: format!("{name}.local"),
            os: Some("linux".into()),
            arch: Some("arm64".into()),
            version: Some("0.1.0".into()),
            wg_public_key: "k".into(),
            overlay_ip: ip.into(),
            status: "active".into(),
            tags: tags.into(),
            labels: "{}".into(),
            nat_type: "unknown".into(),
            relay_connected: 0,
            endpoints: "[]".into(),
            reported_peers: "[]".into(),
            created_at: 0,
            approved_at: Some(0),
            disabled_at: None,
            last_seen_at: Some(0),
            created_by: None,
        }
    }

    #[test]
    fn compiles_tags_and_names() {
        let nodes = vec![
            node("1", "site", "10.66.0.2", r#"["site"]"#),
            node("2", "mac-01", "10.66.0.3", r#"["mac"]"#),
            node("3", "mac-02", "10.66.0.4", r#"["mac"]"#),
        ];
        let policy = AclPolicy {
            version: 1,
            rules: vec![AclRule {
                action: AclAction::Accept,
                src: vec!["tag:site".into()],
                dst: vec!["tag:mac".into()],
                protocol: Some("tcp".into()),
                ports: vec!["22".into(), "8421".into()],
                comment: None,
            }],
        };
        let out = compile_policy(&policy, &nodes, "10.66.0.0/24".parse().unwrap()).unwrap();
        assert_eq!(out.compiled.rules.len(), 2);
        assert!(!out.compiled.default_allow);
        assert!(out.compiled.is_allowed(
            "10.66.0.2".parse().unwrap(),
            "10.66.0.3".parse().unwrap(),
            6,
            Some(22)
        ));
        assert!(!out.compiled.is_allowed(
            "10.66.0.3".parse().unwrap(),
            "10.66.0.4".parse().unwrap(),
            6,
            Some(22)
        ));
    }

    #[test]
    fn unresolved_selectors_fail_closed() {
        for selector in ["tag:missing", "name:missing"] {
            let policy = AclPolicy {
                version: 1,
                rules: vec![AclRule {
                    action: AclAction::Accept,
                    src: vec![selector.into()],
                    dst: vec!["*".into()],
                    protocol: None,
                    ports: vec![],
                    comment: None,
                }],
            };
            let out = compile_policy(&policy, &[], "10.66.0.0/24".parse().unwrap()).unwrap();
            assert!(out.compiled.rules.is_empty());
            assert!(!out.compiled.default_allow);
            assert!(!out.compiled.is_allowed(
                "10.66.0.2".parse().unwrap(),
                "10.66.0.3".parse().unwrap(), 6, Some(8421)
            ));
            assert!(!out.warnings.is_empty());
        }
    }

    #[test]
    fn intentionally_empty_policy_preserves_alpha_allow_all() {
        let policy = AclPolicy { version: 1, rules: vec![] };
        let out = compile_policy(&policy, &[], "10.66.0.0/24".parse().unwrap()).unwrap();
        assert!(out.compiled.default_allow);
    }

    #[test]
    fn simulates() {
        let nodes = vec![
            node("1", "site", "10.66.0.2", r#"["site"]"#),
            node("2", "mac-01", "10.66.0.3", r#"["mac"]"#),
        ];
        let policy = AclPolicy {
            version: 1,
            rules: vec![AclRule {
                action: AclAction::Accept,
                src: vec!["tag:site".into()],
                dst: vec!["tag:mac".into()],
                protocol: Some("tcp".into()),
                ports: vec!["22".into()],
                comment: None,
            }],
        };
        let compiled = compile_policy(&policy, &nodes, "10.66.0.0/24".parse().unwrap())
            .unwrap()
            .compiled;
        let ok = simulate(
            &compiled,
            &nodes,
            &SimulateRequest {
                src: "site".into(),
                dst: "mac-01".into(),
                protocol: Some("tcp".into()),
                port: Some(22),
            },
        )
        .unwrap();
        assert!(ok.allowed);
        let denied = simulate(
            &compiled,
            &nodes,
            &SimulateRequest {
                src: "site".into(),
                dst: "mac-01".into(),
                protocol: Some("tcp".into()),
                port: Some(80),
            },
        )
        .unwrap();
        assert!(!denied.allowed);
    }
}
