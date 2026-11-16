//! Host metrics and discovery collection, independent of HTTP and terminal rendering.
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sysinfo::{Disks, Networks, Pid, Process, ProcessStatus, System};

use crate::discovery::{Discovery, DiscoverySnapshot};
const MAX_TOP_PROCESSES: usize = 8;
const MAX_DISKS: usize = 12;
pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
const DISK_REFRESH_EVERY: u64 = 5;
const DISCOVERY_REFRESH_EVERY: u64 = 5;

// ---------------------------------------------------------------------------
// Snapshot model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub generated_at: DateTime<Utc>,
    pub host: HostInfo,
    pub cpu: CpuInfo,
    pub memory: MemoryInfo,
    pub load: LoadInfo,
    pub disks: Vec<DiskInfo>,
    pub networks: Vec<NetworkInfo>,
    pub wm: Option<ProcessInfo>,
    pub wm_source: &'static str,
    pub processes: Vec<ProcessInfo>,
    pub extensions: DiscoverySnapshot,
}

#[derive(Debug, Clone, Serialize)]
pub struct HostInfo {
    pub hostname: String,
    pub name: String,
    pub os_version: String,
    pub long_os_version: String,
    pub kernel: String,
    pub arch: String,
    pub uptime_secs: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CpuInfo {
    pub brand: String,
    pub cores: usize,
    pub physical_cores: Option<usize>,
    pub frequency_mhz: u64,
    pub usage_percent: f32,
    pub per_core: Vec<f32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryInfo {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub swap_total_bytes: u64,
    pub swap_used_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoadInfo {
    pub one: f64,
    pub five: f64,
    pub fifteen: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiskInfo {
    pub name: String,
    pub mount: String,
    pub file_system: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub usage_percent: f32,
    pub removable: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkInfo {
    pub name: String,
    pub rx_total_bytes: u64,
    pub tx_total_bytes: u64,
    pub rx_per_sec: u64,
    pub tx_per_sec: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProcessInfo {
    pub pid: u32,
    pub parent: Option<u32>,
    pub name: String,
    /// Commands are only exposed for the `wm` agent (the dashboard's subject).
    pub cmd: Option<String>,
    pub state: String,
    pub cpu_percent: f32,
    pub memory_bytes: u64,
    pub virtual_memory_bytes: u64,
    pub run_time_secs: u64,
}

pub struct Sampler {
    system: System,
    disks: Disks,
    networks: Networks,
    discovery: Discovery,
    extensions: DiscoverySnapshot,
    last: Instant,
    ticks: u64,
}

impl Sampler {
    /// Construct the long-running collector with the existing discovery cache.
    pub fn new(root: &Path) -> anyhow::Result<Self> {
        Self::with_discovery_cache(root, true)
    }

    /// Construct a one-shot collector without reading or writing discovery state.
    /// Entries are classified relative to this scan, not historical discoveries.
    pub fn read_only(root: &Path) -> anyhow::Result<Self> {
        Self::with_discovery_cache(root, false)
    }

    fn with_discovery_cache(root: &Path, persist: bool) -> anyhow::Result<Self> {
        let extensions_dir = root.join("extensions");
        let discovery = Discovery::new(
            vec![extensions_dir.clone()],
            persist.then(|| extensions_dir.join(".debug-discovery.json")),
            chrono::Duration::hours(24),
        );
        Ok(Self {
            system: System::new_all().context("sysinfo não conseguiu ler o sistema")?,
            disks: Disks::new_with_refreshed_list().context("sysinfo não conseguiu ler discos")?,
            networks: Networks::new_with_refreshed_list()
                .context("sysinfo não conseguiu ler interfaces")?,
            discovery,
            extensions: DiscoverySnapshot::empty(),
            last: Instant::now(),
            ticks: 0,
        })
    }

    pub fn sample(&mut self) -> Snapshot {
        self.system.refresh_all();
        self.networks.refresh(true);
        if self.ticks % DISK_REFRESH_EVERY == 0 {
            self.disks.refresh(true);
        }
        if self.ticks % DISCOVERY_REFRESH_EVERY == 0 {
            self.extensions = self.discovery.scan();
        }
        self.ticks += 1;

        let now = Instant::now();
        let elapsed = now.duration_since(self.last).as_secs_f64().max(0.001);
        self.last = now;

        let current = sysinfo::get_current_pid().ok();
        let (wm_pid, wm_source) = find_wm_pid(&self.system, current);

        Snapshot {
            generated_at: Utc::now(),
            host: host_info(),
            cpu: cpu_info(&self.system),
            memory: memory_info(&self.system),
            load: load_info(),
            disks: disk_info(&self.disks),
            networks: network_info(&self.networks, elapsed),
            wm: wm_pid
                .and_then(|pid| self.system.process(pid))
                .map(wm_process_info),
            wm_source,
            processes: top_processes(&self.system, current),
            extensions: self.extensions.clone(),
        }
    }
}

fn host_info() -> HostInfo {
    HostInfo {
        hostname: System::host_name().unwrap_or_else(|_| "?".into()),
        name: System::name().unwrap_or_else(|_| "?".into()),
        os_version: System::os_version().unwrap_or_else(|_| "?".into()),
        long_os_version: System::long_os_version().unwrap_or_else(|_| "?".into()),
        kernel: System::kernel_version().unwrap_or_else(|_| "?".into()),
        arch: System::cpu_arch(),
        uptime_secs: System::uptime().unwrap_or(0),
    }
}

fn cpu_info(system: &System) -> CpuInfo {
    let cpus = system.cpus();
    let brand = cpus
        .first()
        .map(|cpu| {
            let brand = cpu.brand().trim();
            if brand.is_empty() {
                cpu.name().to_string()
            } else {
                brand.to_string()
            }
        })
        .unwrap_or_else(|| "CPU desconhecida".into());
    CpuInfo {
        brand,
        cores: cpus.len(),
        physical_cores: System::physical_core_count().ok(),
        frequency_mhz: cpus.first().map(|cpu| cpu.frequency()).unwrap_or(0),
        usage_percent: system.global_cpu_usage(),
        per_core: cpus.iter().map(|cpu| cpu.usage()).collect(),
    }
}

fn memory_info(system: &System) -> MemoryInfo {
    MemoryInfo {
        total_bytes: system.total_memory(),
        used_bytes: system.used_memory(),
        available_bytes: system.available_memory(),
        swap_total_bytes: system.total_swap(),
        swap_used_bytes: system.used_swap(),
    }
}

fn load_info() -> LoadInfo {
    match System::load_average() {
        Ok(load) => LoadInfo {
            one: load.one,
            five: load.five,
            fifteen: load.fifteen,
        },
        Err(_) => LoadInfo {
            one: 0.0,
            five: 0.0,
            fifteen: 0.0,
        },
    }
}

fn disk_info(disks: &Disks) -> Vec<DiskInfo> {
    let mut out: Vec<DiskInfo> = disks
        .iter()
        .filter(|disk| disk.total_space() > 0)
        .map(|disk| {
            let total = disk.total_space();
            let available = disk.available_space();
            let used = total.saturating_sub(available);
            DiskInfo {
                name: disk.name().to_string_lossy().into_owned(),
                mount: disk.mount_point().display().to_string(),
                file_system: disk.file_system().to_string_lossy().into_owned(),
                total_bytes: total,
                used_bytes: used,
                available_bytes: available,
                usage_percent: percent(used, total),
                removable: disk.is_removable(),
            }
        })
        .collect();
    out.sort_by(|a, b| a.mount.cmp(&b.mount));
    out.truncate(MAX_DISKS);
    out
}

fn network_info(networks: &Networks, elapsed: f64) -> Vec<NetworkInfo> {
    let mut out: Vec<NetworkInfo> = networks
        .iter()
        .map(|(name, data)| NetworkInfo {
            name: name.to_string_lossy().into_owned(),
            rx_total_bytes: data.total_received(),
            tx_total_bytes: data.total_transmitted(),
            rx_per_sec: (data.received() as f64 / elapsed).round() as u64,
            tx_per_sec: (data.transmitted() as f64 / elapsed).round() as u64,
        })
        // Idle virtual interfaces add noise; show what actually moved bytes.
        .filter(|net| net.rx_total_bytes + net.tx_total_bytes > 0)
        .collect();
    out.sort_by(|a, b| {
        (b.rx_total_bytes + b.tx_total_bytes).cmp(&(a.rx_total_bytes + a.tx_total_bytes))
    });
    out.truncate(12);
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn top_processes(system: &System, current: Option<Pid>) -> Vec<ProcessInfo> {
    let mut processes: Vec<&Process> = system
        .processes()
        .iter()
        .filter(|(pid, _)| Some(**pid) != current)
        .map(|(_, process)| process)
        .collect();
    processes.sort_by(|a, b| {
        b.cpu_usage()
            .partial_cmp(&a.cpu_usage())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.memory().cmp(&a.memory()))
    });
    processes
        .into_iter()
        .take(MAX_TOP_PROCESSES)
        .map(process_info)
        .collect()
}

fn process_info(process: &Process) -> ProcessInfo {
    ProcessInfo {
        pid: process.pid().as_u32(),
        parent: process.parent().map(|pid| pid.as_u32()),
        name: process.name().to_string_lossy().into_owned(),
        cmd: None,
        state: format!("{:?}", process.status()).to_lowercase(),
        cpu_percent: process.cpu_usage(),
        memory_bytes: process.memory(),
        virtual_memory_bytes: process.virtual_memory(),
        run_time_secs: process.run_time(),
    }
}

fn wm_process_info(process: &Process) -> ProcessInfo {
    let mut info = process_info(process);
    let command = process
        .cmd()
        .iter()
        .map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");
    if !command.is_empty() {
        info.cmd = Some(command.chars().take(400).collect());
    }
    info
}

fn find_wm_pid(system: &System, current: Option<Pid>) -> (Option<Pid>, &'static str) {
    let Some(current) = current else {
        return (None, "unavailable");
    };
    // The exec supervisor starts the sidecar directly, so the parent process is
    // normally the `wm` agent. This is the strongest and cheapest signal.
    if let Some(parent) = system.process(current).and_then(|process| process.parent()) {
        if system
            .process(parent)
            .is_some_and(|process| is_wm_process(process))
        {
            return (Some(parent), "parent");
        }
    }

    // Manual run (`wm debug` from a shell): score wm processes and prefer the
    // long-running agent over short-lived CLI invocations.
    let mut best: Option<(i32, u64, Pid)> = None;
    for (pid, process) in system.processes() {
        if *pid == current || !is_wm_process(process) {
            continue;
        }
        let command = process
            .cmd()
            .iter()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        let score = if [" up", " join", " host"]
            .iter()
            .any(|needle| command.contains(needle))
        {
            3
        } else if [" debug", " chat", " tui", " gui"]
            .iter()
            .any(|needle| command.contains(needle))
        {
            0
        } else {
            1
        };
        let candidate = (score, process.run_time(), *pid);
        let better = match best {
            None => true,
            Some((best_score, best_run, _)) => {
                candidate.0 > best_score || (candidate.0 == best_score && candidate.1 > best_run)
            }
        };
        if better {
            best = Some(candidate);
        }
    }
    (best.map(|(_, _, pid)| pid), "scan")
}

fn is_wm_process(process: &Process) -> bool {
    // A dead/zombie entry can still match by name and would report zeros.
    if process.memory() == 0 || matches!(process.status(), ProcessStatus::Zombie | ProcessStatus::Dead)
    {
        return false;
    }
    let name = process.name().to_string_lossy();
    if name == "wm" || name.ends_with("/wm") {
        return true;
    }
    process
        .exe()
        .and_then(|path| path.file_name())
        .is_some_and(|name| name.to_string_lossy() == "wm")
}

fn percent(part: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        (part as f64 / total as f64 * 100.0) as f32
    }
}

#[cfg(test)]
mod tests { use super::*;
    #[test]
    fn percent_handles_zero_and_rounds() {
        assert_eq!(percent(0, 0), 0.0);
        assert_eq!(percent(1, 2), 50.0);
        assert_eq!(percent(2, 2), 100.0);
    }
}

pub(crate) mod terminal;
