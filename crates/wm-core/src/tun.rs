//! TUN device creation and OS route management.
//!
//! The agent creates an L3 device, assigns the overlay /32 and installs a route
//! for the whole overlay CIDR. On macOS we disable the tun crate's automatic
//! route and use an interface route, which is what makes `route -n add -net ...
//! -interface utunN` stable across reboots.

use std::net::Ipv4Addr;
use std::process::Command;

use anyhow::{Context, bail};
use ipnet::Ipv4Net;
use tokio_util::codec::Framed;
use tun::{AbstractDevice, AsyncDevice, TunPacketCodec};

use crate::state::AgentConfig;

pub type TunFramed = Framed<AsyncDevice, TunPacketCodec>;

pub struct TunDevice {
    pub name: String,
    pub framed: TunFramed,
}

struct RouteSpec {
    name: String,
    ip: Ipv4Addr,
    cidr: Ipv4Net,
}

#[cfg(target_os = "linux")]
impl RouteSpec {
    fn add(&self) -> anyhow::Result<()> {
        run(
            "ip",
            &["addr", "replace", &format!("{}/32", self.ip), "dev", &self.name],
        )?;
        run(
            "ip",
            &["link", "set", "dev", &self.name, "up", "mtu", "1280"],
        )?;
        run(
            "ip",
            &["route", "replace", &self.cidr.to_string(), "dev", &self.name],
        )?;
        Ok(())
    }
    fn remove(&self) {
        let _ = run(
            "ip",
            &["route", "del", &self.cidr.to_string(), "dev", &self.name],
        );
        let _ = run(
            "ip",
            &["addr", "del", &format!("{}/32", self.ip), "dev", &self.name],
        );
    }
}

#[cfg(target_os = "macos")]
impl RouteSpec {
    fn add(&self) -> anyhow::Result<()> {
        run(
            "ifconfig",
            &[
                &self.name,
                "inet",
                &self.ip.to_string(),
                &self.ip.to_string(),
                "up",
                "mtu",
                "1280",
            ],
        )?;
        match run(
            "route",
            &[
                "-n",
                "add",
                "-net",
                &self.cidr.to_string(),
                "-interface",
                &self.name,
            ],
        ) {
            Ok(()) => Ok(()),
            Err(err) => {
                // An existing route with the same destination is fine; the
                // interface may have been reused after a crash.
                let msg = format!("{err}");
                if msg.contains("File exists") {
                    Ok(())
                } else {
                    Err(err)
                }
            }
        }
    }
    fn remove(&self) {
        let _ = run(
            "route",
            &[
                "-n",
                "delete",
                "-net",
                &self.cidr.to_string(),
                "-interface",
                &self.name,
            ],
        );
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
impl RouteSpec {
    fn add(&self) -> anyhow::Result<()> {
        bail!("plataforma sem suporte a TUN nesta versão")
    }
    fn remove(&self) {}
}

pub struct RouteGuard {
    spec: RouteSpec,
}

impl Drop for RouteGuard {
    fn drop(&mut self) {
        self.spec.remove();
    }
}

pub fn create(
    cfg: &AgentConfig,
    overlay_ip: Ipv4Addr,
    cidr: Ipv4Net,
) -> anyhow::Result<(TunDevice, RouteGuard)> {
    let mut config = tun::Configuration::default();
    config.layer(tun::Layer::L3).mtu(cfg.mtu).up();
    #[cfg(target_os = "macos")]
    {
        config.platform_config(|p| {
            p.enable_routing(false);
            p.packet_information(true);
        });
    }
    #[cfg(target_os = "linux")]
    {
        config.platform_config(|p| {
            p.ensure_root_privileges(true);
        });
    }
    if let Some(name) = &cfg.tun_name {
        config.tun_name(name);
    }

    let dev = tun::create_as_async(&config).context(
        "não foi possível criar o TUN (rode como root e verifique /dev/net/tun ou utun)",
    )?;
    let name = dev.tun_name().unwrap_or_else(|_| "tun".to_string());
    let spec = RouteSpec {
        name: name.clone(),
        ip: overlay_ip,
        cidr,
    };
    spec.add()?;
    tracing::info!(%name, %overlay_ip, %cidr, "TUN configurado");
    let guard = RouteGuard { spec };
    Ok((
        TunDevice {
            name,
            framed: dev.into_framed(),
        },
        guard,
    ))
}

fn run(bin: &str, args: &[&str]) -> anyhow::Result<()> {
    let out = Command::new(bin)
        .args(args)
        .output()
        .with_context(|| format!("executando {bin} {}", args.join(" ")))?;
    if !out.status.success() {
        bail!(
            "{} {} falhou: {}",
            bin,
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}
