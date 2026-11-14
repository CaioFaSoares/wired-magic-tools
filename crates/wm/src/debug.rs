//! Public debug dashboard (`wm debug`).
//!
//! The `debug` extension in `extensions/public/debug` runs this as an `exec`
//! sidecar. It samples host performance and the `wm` agent process once per
//! second and serves a small read-only dashboard, plus `/api/snapshot` for
//! automation.
//!
//! It is deliberately local-only: the manifest binds `127.0.0.1:8791` and the
//! adapter exposes that loopback port on the mesh. The payload includes
//! hostnames, disk layout, interface names and process metrics, which is
//! sensitive enough to keep it off the public internet. There is no auth, so
//! treat the mesh ACL as the access control.
//!
//! The sampling thread owns `sysinfo` state and writes immutable snapshots to
//! an `RwLock`. Axum handlers only clone the latest snapshot, so a slow client
//! never blocks collection.

use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use anyhow::Context;
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{Value, json};
use sysinfo::{Disks, Networks, Pid, Process, ProcessStatus, System};

const INDEX_HTML: &str = include_str!("debug_assets/index.html");
const APP_CSS: &str = include_str!("debug_assets/app.css");
const APP_JS: &str = include_str!("debug_assets/app.js");

const MAX_TOP_PROCESSES: usize = 8;
const MAX_DISKS: usize = 12;
const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
const DISK_REFRESH_EVERY: u64 = 5;

// ---------------------------------------------------------------------------
// Snapshot model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
struct Snapshot {
    generated_at: DateTime<Utc>,
    host: HostInfo,
    cpu: CpuInfo,
    memory: MemoryInfo,
    load: LoadInfo,
    disks: Vec<DiskInfo>,
    networks: Vec<NetworkInfo>,
    wm: Option<ProcessInfo>,
    wm_source: &'static str,
    processes: Vec<ProcessInfo>,
}

#[derive(Debug, Clone, Serialize)]
struct HostInfo {
    hostname: String,
    name: String,
    os_version: String,
    long_os_version: String,
    kernel: String,
    arch: String,
    uptime_secs: u64,
}

#[derive(Debug, Clone, Serialize)]
struct CpuInfo {
    brand: String,
    cores: usize,
    physical_cores: Option<usize>,
    frequency_mhz: u64,
    usage_percent: f32,
    per_core: Vec<f32>,
}

#[derive(Debug, Clone, Serialize)]
struct MemoryInfo {
    total_bytes: u64,
    used_bytes: u64,
    available_bytes: u64,
    swap_total_bytes: u64,
    swap_used_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
struct LoadInfo {
    one: f64,
    five: f64,
    fifteen: f64,
}

#[derive(Debug, Clone, Serialize)]
struct DiskInfo {
    name: String,
    mount: String,
    file_system: String,
    total_bytes: u64,
    used_bytes: u64,
    available_bytes: u64,
    usage_percent: f32,
    removable: bool,
}

#[derive(Debug, Clone, Serialize)]
struct NetworkInfo {
    name: String,
    rx_total_bytes: u64,
    tx_total_bytes: u64,
    rx_per_sec: u64,
    tx_per_sec: u64,
}

#[derive(Debug, Clone, Serialize)]
struct ProcessInfo {
    pid: u32,
    parent: Option<u32>,
    name: String,
    /// Commands are only exposed for the `wm` agent (the dashboard's subject).
    cmd: Option<String>,
    state: String,
    cpu_percent: f32,
    memory_bytes: u64,
    virtual_memory_bytes: u64,
    run_time_secs: u64,
}

#[derive(Debug, Serialize)]
struct InfoResponse {
    title: String,
    node: Option<String>,
    version: String,
    interval_secs: u64,
}

// ---------------------------------------------------------------------------
// State and sampler
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct DebugState {
    title: String,
    node: Option<String>,
    snapshot: Arc<RwLock<Option<Arc<Snapshot>>>>,
}

impl DebugState {
    fn new(title: String, node: Option<String>, first: Snapshot) -> Self {
        Self {
            title,
            node,
            snapshot: Arc::new(RwLock::new(Some(Arc::new(first)))),
        }
    }

    fn set(&self, snapshot: Snapshot) {
        if let Ok(mut current) = self.snapshot.write() {
            *current = Some(Arc::new(snapshot));
        }
    }

    fn get(&self) -> Option<Arc<Snapshot>> {
        self.snapshot
            .read()
            .ok()
            .and_then(|current| current.clone())
    }
}

struct Sampler {
    system: System,
    disks: Disks,
    networks: Networks,
    last: Instant,
    ticks: u64,
}

impl Sampler {
    fn new() -> anyhow::Result<Self> {
        Ok(Self {
            system: System::new_all().context("sysinfo não conseguiu ler o sistema")?,
            disks: Disks::new_with_refreshed_list().context("sysinfo não conseguiu ler discos")?,
            networks: Networks::new_with_refreshed_list()
                .context("sysinfo não conseguiu ler interfaces")?,
            last: Instant::now(),
            ticks: 0,
        })
    }

    fn sample(&mut self) -> Snapshot {
        self.system.refresh_all();
        self.networks.refresh(true);
        if self.ticks % DISK_REFRESH_EVERY == 0 {
            self.disks.refresh(true);
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

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; img-src 'self' data:; style-src 'self'; script-src 'self'; \
             connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
        ),
    );
    response
}

fn asset(body: &'static str, content_type: &'static str) -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
        ],
        body,
    )
        .into_response()
}

async fn index() -> Response {
    asset(INDEX_HTML, "text/html; charset=utf-8")
}

async fn css() -> Response {
    asset(APP_CSS, "text/css; charset=utf-8")
}

async fn js() -> Response {
    asset(APP_JS, "text/javascript; charset=utf-8")
}

async fn health() -> Json<Value> {
    Json(json!({ "ok": true, "service": "wm-debug" }))
}

async fn info(State(state): State<DebugState>) -> Json<InfoResponse> {
    Json(InfoResponse {
        title: state.title.clone(),
        node: state.node.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        interval_secs: SAMPLE_INTERVAL.as_secs(),
    })
}

async fn snapshot(State(state): State<DebugState>) -> Response {
    match state.get() {
        Some(snapshot) => Json((*snapshot).clone()).into_response(),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "coletando a primeira amostra" })),
        )
            .into_response(),
    }
}

async fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "error": "rota não encontrada" })),
    )
        .into_response()
}

fn app(state: DebugState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/index.html", get(index))
        .route("/app.css", get(css))
        .route("/app.js", get(js))
        .route("/healthz", get(health))
        .route("/api/info", get(info))
        .route("/api/snapshot", get(snapshot))
        .fallback(not_found)
        .layer(DefaultBodyLimit::max(8 * 1024))
        .layer(middleware::from_fn(security_headers))
        .with_state(state)
}

/// Starts the sampler and serves the dashboard until the process is stopped.
pub async fn run(
    listen: SocketAddr,
    title: String,
    node: Option<String>,
) -> anyhow::Result<()> {
    let mut sampler = Sampler::new()?;
    let first = sampler.sample();
    let state = DebugState::new(title, node, first);

    let sampler_state = state.clone();
    std::thread::Builder::new()
        .name("wm-debug-sampler".into())
        .spawn(move || {
            loop {
                std::thread::sleep(SAMPLE_INTERVAL);
                sampler_state.set(sampler.sample());
            }
        })
        .context("não foi possível iniciar o coletor de métricas")?;

    let app = app(state);
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("não foi possível escutar em {listen}"))?;
    println!("debug: ouvindo em http://{listen}");
    axum::serve(listener, app)
        .await
        .context("servidor de debug encerrou com erro")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use serde_json::Value as JsonValue;
    use tower::ServiceExt;

    fn test_state() -> DebugState {
        // A tiny synthetic snapshot keeps the HTTP tests independent from the
        // host's real load.
        let snapshot = SurrogateSnapshot::build();
        DebugState::new("Debug de teste".into(), Some("nó-1".into()), snapshot)
    }

    struct SurrogateSnapshot;

    impl SurrogateSnapshot {
        fn build() -> Snapshot {
            Snapshot {
                generated_at: Utc::now(),
                host: HostInfo {
                    hostname: "host".into(),
                    name: "OS".into(),
                    os_version: "1".into(),
                    long_os_version: "OS 1".into(),
                    kernel: "kernel".into(),
                    arch: "arm64".into(),
                    uptime_secs: 42,
                },
                cpu: CpuInfo {
                    brand: "CPU".into(),
                    cores: 2,
                    physical_cores: Some(2),
                    frequency_mhz: 1000,
                    usage_percent: 10.0,
                    per_core: vec![10.0, 10.0],
                },
                memory: MemoryInfo {
                    total_bytes: 100,
                    used_bytes: 40,
                    available_bytes: 60,
                    swap_total_bytes: 0,
                    swap_used_bytes: 0,
                },
                load: LoadInfo {
                    one: 0.1,
                    five: 0.1,
                    fifteen: 0.1,
                },
                disks: Vec::new(),
                networks: Vec::new(),
                wm: None,
                wm_source: "unavailable",
                processes: Vec::new(),
            }
        }
    }

    async fn body_json(response: Response) -> JsonValue {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[test]
    fn percent_handles_zero_and_rounds() {
        assert_eq!(percent(0, 0), 0.0);
        assert_eq!(percent(1, 2), 50.0);
        assert_eq!(percent(2, 2), 100.0);
    }

    #[tokio::test]
    async fn dashboard_and_snapshot_are_served_with_security_headers() {
        let app = app(test_state());

        let response = app
            .clone()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
        assert_eq!(
            response.headers()[header::X_CONTENT_TYPE_OPTIONS],
            "nosniff"
        );
        assert!(
            response
                .headers()
                .get(header::CONTENT_SECURITY_POLICY)
                .is_some()
        );

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/info")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let info = body_json(response).await;
        assert_eq!(info["title"], "Debug de teste");
        assert_eq!(info["node"], "nó-1");
        assert_eq!(info["interval_secs"], 1);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/snapshot")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let snapshot = body_json(response).await;
        assert_eq!(snapshot["host"]["hostname"], "host");
        assert_eq!(snapshot["cpu"]["cores"], 2);
        assert_eq!(snapshot["memory"]["used_bytes"], 40);
    }
}
