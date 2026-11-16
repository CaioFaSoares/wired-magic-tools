//! Terminal-oriented local collection. Keeps the existing process selection semantics.
use std::path::Path;
use std::time::{Duration, Instant};
use sysinfo::{Process, ProcessStatus, System};

use crate::discovery::{Discovery, DiscoverySnapshot};

pub(crate) const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
const DISCOVERY_INTERVAL: Duration = Duration::from_secs(5);
const MAX_PROCESSES: usize = 7;
pub(crate) struct ProcRow {
    pub(crate) pid: u32,
    pub(crate) name: String,
    pub(crate) cpu: f32,
    pub(crate) memory: u64,
    pub(crate) state: String,
}

pub(crate) struct State {
    pub(crate) system: System,
    pub(crate) hostname: String,
    pub(crate) os: String,
    pub(crate) uptime_secs: u64,
    pub(crate) cpu: f32,
    pub(crate) per_core: Vec<f32>,
    pub(crate) memory_total: u64,
    pub(crate) memory_used: u64,
    pub(crate) load_one: f64,
    pub(crate) load_five: f64,
    pub(crate) load_fifteen: f64,
    pub(crate) wm: Option<ProcRow>,
    pub(crate) wm_parent: Option<ProcRow>,
    pub(crate) processes: Vec<ProcRow>,
    pub(crate) discovery: Discovery,
    pub(crate) extensions: DiscoverySnapshot,
    pub(crate) last_sample: Instant,
    pub(crate) last_discovery: Instant,
    pub(crate) status: String,
}

impl State {
    pub(crate) fn new(root: &Path) -> Self {
        let extensions_dir = root.join("extensions");
        let discovery = Discovery::new(
            vec![extensions_dir.clone()],
            Some(extensions_dir.join(".debug-discovery.json")),
            chrono::Duration::hours(24),
        );
        Self {
            system: System::new_all().unwrap_or_else(|_| System::new().expect("sysinfo")),
            hostname: System::host_name().unwrap_or_else(|_| "?".into()),
            os: System::long_os_version()
                .or_else(|_| System::name())
                .unwrap_or_else(|_| "?".into()),
            uptime_secs: 0,
            cpu: 0.0,
            per_core: Vec::new(),
            memory_total: 0,
            memory_used: 0,
            load_one: 0.0,
            load_five: 0.0,
            load_fifteen: 0.0,
            wm: None,
            wm_parent: None,
            processes: Vec::new(),
            discovery,
            extensions: DiscoverySnapshot::empty(),
            last_sample: Instant::now() - SAMPLE_INTERVAL,
            last_discovery: Instant::now() - DISCOVERY_INTERVAL,
            status: "coletando…".into(),
        }
    }

    pub(crate) fn sample(&mut self) {
        self.system.refresh_all();
        self.hostname = System::host_name().unwrap_or_else(|_| "?".into());
        self.os = System::long_os_version()
            .or_else(|_| System::name())
            .unwrap_or_else(|_| "?".into());
        self.uptime_secs = System::uptime().unwrap_or(0);
        self.cpu = self.system.global_cpu_usage();
        self.per_core = self.system.cpus().iter().map(|cpu| cpu.usage()).collect();
        self.memory_total = self.system.total_memory();
        self.memory_used = self.system.used_memory();
        if let Ok(load) = System::load_average() {
            self.load_one = load.one;
            self.load_five = load.five;
            self.load_fifteen = load.fifteen;
        }

        let current = sysinfo::get_current_pid().ok();
        self.wm = current
            .and_then(|pid| self.system.process(pid))
            .map(process_row);
        self.wm_parent = current
            .and_then(|pid| self.system.process(pid))
            .and_then(|process| process.parent())
            .and_then(|pid| self.system.process(pid))
            .filter(|process| is_wm_process(process))
            .map(process_row);

        let mut processes: Vec<&Process> = self.system.processes().values().collect();
        processes.sort_by(|a, b| {
            b.cpu_usage()
                .partial_cmp(&a.cpu_usage())
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.memory().cmp(&a.memory()))
        });
        self.processes = processes
            .into_iter()
            .take(MAX_PROCESSES)
            .map(process_row)
            .collect();

        if self.last_discovery.elapsed() >= DISCOVERY_INTERVAL {
            self.extensions = self.discovery.scan();
            self.last_discovery = Instant::now();
        }
        self.last_sample = Instant::now();
        self.status = format!(
            "amostra {} · {} extensão(ões) local(is), {} nova(s)",
            chrono::Local::now().format("%H:%M:%S"),
            self.extensions.entries.len(),
            self.extensions.new_count
        );
    }
}

fn process_row(process: &Process) -> ProcRow {
    ProcRow {
        pid: process.pid().as_u32(),
        name: process.name().to_string_lossy().into_owned(),
        cpu: process.cpu_usage(),
        memory: process.memory(),
        state: format!("{:?}", process.status()).to_lowercase(),
    }
}

fn is_wm_process(process: &Process) -> bool {
    if process.memory() == 0
        || matches!(process.status(), ProcessStatus::Zombie | ProcessStatus::Dead)
    {
        return false;
    }
    let name = process.name().to_string_lossy();
    name == "wm"
        || name.ends_with("/wm")
        || process
            .exe()
            .and_then(|path| path.file_name())
            .is_some_and(|name| name.to_string_lossy() == "wm")
}
