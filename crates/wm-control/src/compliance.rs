//! Evaluation of per-node requirements.
//!
//! Requirements are declarative and stored on the node row. The control plane
//! recomputes compliance from *observed* data (last report plus extension
//! status) instead of trusting the member to self-report "compliant". Checks
//! are advisory in this version: a non-compliant node keeps working and is
//! flagged in the map, TUI and web panel.

use chrono::Utc;

use wm_proto::{
    AgentRequirements, AgentRuntime, ExtensionState, ExtensionStatus, NodeCompliance,
    NodeRequirements, RequirementCheck,
};

/// Evaluates every configured requirement. An empty requirement set is
/// trivially compliant (invitation-only policy remains the default).
pub fn evaluate(
    actual_version: &str,
    tags: &[String],
    runtime: &AgentRuntime,
    extensions: &[ExtensionStatus],
    req: &NodeRequirements,
) -> NodeCompliance {
    let mut checks = Vec::new();

    if let Some(min) = req.min_version.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        let (ok, detail) = version_satisfies(min, actual_version);
        checks.push(RequirementCheck {
            id: "min_version".into(),
            label: format!("versão ≥ {min}"),
            ok,
            detail,
        });
    }

    for tag in &req.required_tags {
        let ok = tags.iter().any(|t| t == tag);
        checks.push(RequirementCheck {
            id: format!("tag:{tag}"),
            label: format!("tag `{tag}`"),
            ok,
            detail: if ok {
                "tag presente".into()
            } else {
                "tag ausente no nó".into()
            },
        });
    }

    for id in &req.required_extensions {
        let status = extensions.iter().find(|status| &status.id == id);
        let (ok, detail) = match status {
            Some(status) if status.state == ExtensionState::Running => {
                (true, "extensão ativa".to_string())
            }
            Some(status) => (
                false,
                format!(
                    "extensão em {}{}",
                    status.state,
                    status
                        .message
                        .as_deref()
                        .filter(|m| !m.is_empty())
                        .map(|m| format!(": {m}"))
                        .unwrap_or_default()
                ),
            ),
            None => (false, "extensão não reportada pelo nó".to_string()),
        };
        checks.push(RequirementCheck {
            id: format!("extension:{id}"),
            label: format!("extensão `{id}` ativa"),
            ok,
            detail,
        });
    }

    evaluate_agent(&req.agent, runtime, &mut checks);

    NodeCompliance {
        compliant: checks.iter().all(|check| check.ok),
        checked_at: Some(Utc::now()),
        checks,
    }
}

fn evaluate_agent(req: &AgentRequirements, runtime: &AgentRuntime, checks: &mut Vec<RequirementCheck>) {
    if let Some(expected) = req.udp_port {
        let ok = runtime.udp_port == Some(expected);
        checks.push(RequirementCheck {
            id: "agent.udp_port".into(),
            label: format!("porta UDP {expected}"),
            ok,
            detail: match runtime.udp_port {
                Some(actual) => format!("agente usa {actual}"),
                None => "agente não reportou a porta UDP".into(),
            },
        });
    }
    if let Some(expected) = req.mtu {
        let ok = runtime.mtu == Some(expected);
        checks.push(RequirementCheck {
            id: "agent.mtu".into(),
            label: format!("MTU {expected}"),
            ok,
            detail: match runtime.mtu {
                Some(actual) => format!("agente usa {actual}"),
                None => "agente não reportou o MTU".into(),
            },
        });
    }
    if let Some(expected) = req.force_relay {
        let ok = runtime.force_relay == Some(expected);
        checks.push(RequirementCheck {
            id: "agent.force_relay".into(),
            label: format!("forçar relay = {expected}"),
            ok,
            detail: match runtime.force_relay {
                Some(actual) => format!("agente usa {actual}"),
                None => "agente não reportou o modo relay".into(),
            },
        });
    }
    if let Some(expected) = &req.expose {
        let mut expected: Vec<String> = expected.iter().filter_map(|spec| normalize_expose(spec)).collect();
        let mut actual: Vec<String> = runtime.expose.iter().filter_map(|spec| normalize_expose(spec)).collect();
        expected.sort();
        expected.dedup();
        actual.sort();
        actual.dedup();
        let ok = expected == actual;
        checks.push(RequirementCheck {
            id: "agent.expose".into(),
            label: "portas expostas".into(),
            ok,
            detail: if ok {
                format!("{} exposta(s)", actual.len())
            } else {
                format!(
                    "esperado [{}], agente reporta [{}]",
                    expected.join(", "),
                    actual.join(", ")
                )
            },
        });
    }
    if let Some(expected) = req.tun_name.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        let ok = runtime.tun_name.as_deref() == Some(expected);
        checks.push(RequirementCheck {
            id: "agent.tun_name".into(),
            label: format!("TUN `{expected}`"),
            ok,
            detail: match runtime.tun_name.as_deref() {
                Some(actual) => format!("agente usa `{actual}`"),
                None => "agente não reportou o TUN".into(),
            },
        });
    }
}

/// Compares semver when both sides parse; otherwise requires equality so a
/// malformed requirement fails closed instead of silently passing.
fn version_satisfies(min: &str, actual: &str) -> (bool, String) {
    let min = min.trim().trim_start_matches(['v', 'V']);
    let actual = actual.trim().trim_start_matches(['v', 'V']);
    if actual.is_empty() {
        return (false, "agente não reportou a versão".into());
    }
    match (
        semver::Version::parse(min),
        semver::Version::parse(actual),
    ) {
        (Ok(min), Ok(actual)) => {
            let ok = actual >= min;
            (ok, format!("agente {actual}, mínimo {min}"))
        }
        _ => {
            let ok = actual == min;
            (ok, format!("agente `{actual}`, mínimo `{min}` (comparação textual)"))
        }
    }
}

/// Normalizes `porta` and `overlay:local` specs so the comparison ignores
/// formatting differences.
pub fn normalize_expose(spec: &str) -> Option<String> {
    let raw = spec.trim();
    if raw.is_empty() {
        return None;
    }
    let (overlay, local) = raw.split_once(':').unwrap_or((raw, raw));
    let overlay: u16 = overlay.trim().parse().ok()?;
    let local: u16 = local.trim().parse().ok()?;
    if overlay == 0 || local == 0 {
        return None;
    }
    Some(if overlay == local {
        overlay.to_string()
    } else {
        format!("{overlay}:{local}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn status(id: &str, state: ExtensionState) -> ExtensionStatus {
        ExtensionStatus {
            id: id.into(),
            name: id.into(),
            driver: "sidecar".into(),
            state,
            message: None,
            public_urls: Vec::new(),
            details: serde_json::Value::Null,
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn empty_requirements_are_compliant() {
        let report = evaluate("0.1.3", &[], &AgentRuntime::default(), &[], &NodeRequirements::default());
        assert!(report.compliant);
        assert!(report.checks.is_empty());
    }

    #[test]
    fn reports_each_violation_without_stopping_the_node() {
        let requirements = NodeRequirements {
            min_version: Some("0.2.0".into()),
            required_tags: vec!["site".into()],
            required_extensions: vec!["cloudflared".into()],
            agent: AgentRequirements {
                mtu: Some(1400),
                expose: Some(vec!["3000".into()]),
                ..Default::default()
            },
        };
        let runtime = AgentRuntime {
            mtu: Some(1280),
            expose: vec!["3000:3000".into()],
            ..Default::default()
        };
        let report = evaluate(
            "0.1.3",
            &["mac".into()],
            &runtime,
            &[status("cloudflared", ExtensionState::Error)],
            &requirements,
        );
        assert!(!report.compliant);
        let failing: Vec<&str> = report
            .checks
            .iter()
            .filter(|check| !check.ok)
            .map(|check| check.id.as_str())
            .collect();
        assert!(failing.contains(&"min_version"));
        assert!(failing.contains(&"tag:site"));
        assert!(failing.contains(&"extension:cloudflared"));
        assert!(failing.contains(&"agent.mtu"));
        // `3000` and `3000:3000` are the same requirement.
        assert!(report.checks.iter().any(|check| check.id == "agent.expose" && check.ok));
    }

    #[test]
    fn version_uses_semver_and_fails_closed_on_garbage() {
        assert!(version_satisfies("0.2.0", "0.2.0").0);
        assert!(version_satisfies("v0.2.0", "0.10.0").0);
        assert!(!version_satisfies("0.2.0", "0.1.9").0);
        assert!(!version_satisfies("0.2.0", "").0);
        assert!(version_satisfies("nightly", "nightly").0);
        assert!(!version_satisfies("nightly", "0.1.0").0);
    }
}
