//! Detached agent and background-job management.
//!
//! `wm tui` must not take the mesh down when it closes, so lifecycle commands
//! are started in a new session (`setsid`), with stdin closed and stdout/stderr
//! redirected to a private log file. Closing the TUI only closes the view; the
//! process keeps running until `wm down` (or the stop action inside the TUI).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use crate::Msg;
use crate::persist::AgentSettings;

const LOG_ROTATE_BYTES: u64 = 8 * 1024 * 1024;
const TAIL_BYTES: u64 = 96 * 1024;
const TAIL_LINES: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentEvent {
    /// The detached process exists; the local socket is not up yet.
    Starting { pid: u32 },
    Started { pid: u32 },
    StartFailed {
        pid: u32,
        code: Option<i32>,
        tail: Vec<String>,
    },
    Exited { pid: u32, code: Option<i32> },
    Stopped { pid: u32 },
    StopTimeout { pid: u32 },
    StopFailed { pid: u32, error: String },
    NoAgent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobEvent {
    Started {
        pid: u32,
        label: String,
        log: PathBuf,
    },
    Exited {
        pid: u32,
        label: String,
        code: Option<i32>,
    },
    StopTimeout {
        pid: u32,
        label: String,
    },
}

fn errno() -> i32 {
    io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// Only missing or refused sockets prove absence. Permission and other errors
/// must remain errors: they do not establish that a daemon is stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketProbe {
    Listening,
    Absent,
}

pub fn probe_socket(socket: &Path) -> io::Result<SocketProbe> {
    match std::os::unix::net::UnixStream::connect(socket) {
        Ok(_) => Ok(SocketProbe::Listening),
        Err(err) if matches!(err.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused) => {
            Ok(SocketProbe::Absent)
        }
        Err(err) => Err(err),
    }
}

pub fn socket_ready(socket: &Path) -> bool {
    matches!(probe_socket(socket), Ok(SocketProbe::Listening))
}

pub fn read_pid(root: &Path) -> Option<u32> {
    fs::read_to_string(root.join("agent.pid"))
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok())
        .filter(|pid| *pid > 0)
}

/// True when another process currently holds the data-dir agent lock. The
/// agent holds it from start to shutdown, so it is a stronger liveness signal
/// than a pid file that may be stale. Inaccessible locks and unknown errors
/// are treated as busy, never as permission to clean up a running agent.
pub fn agent_lock_held(root: &Path) -> bool {
    use std::os::fd::AsRawFd;
    let file = match OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join("agent.lock"))
    {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return false,
        // Inaccessible or otherwise unverifiable locks are conservatively busy.
        Err(_) => return true,
    };
    let fd = file.as_raw_fd();
    let result = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        unsafe {
            libc::flock(fd, libc::LOCK_UN);
        }
        false
    } else {
        true
    }
}

pub fn pid_alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || errno() == libc::EPERM
}

pub fn signal(pid: u32, signal: i32) -> io::Result<()> {
    if pid == 0 || pid > i32::MAX as u32 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "pid inválido"));
    }
    let result = unsafe { libc::kill(pid as libc::pid_t, signal) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn rotate_log(path: &Path) {
    let Ok(meta) = fs::metadata(path) else {
        return;
    };
    if meta.len() <= LOG_ROTATE_BYTES {
        return;
    }
    let backup = path.with_extension("log.1");
    let _ = fs::remove_file(&backup);
    let _ = fs::rename(path, backup);
}

/// Last lines of a log file, reading only its tail.
pub fn tail(path: &Path, max_bytes: u64) -> Vec<String> {
    let Ok(mut file) = File::open(path) else {
        return Vec::new();
    };
    let len = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    let skip_partial = len > max_bytes;
    if skip_partial {
        let _ = file.seek(SeekFrom::Start(len - max_bytes));
    }
    let mut buffer = Vec::new();
    if file.read_to_end(&mut buffer).is_err() {
        return Vec::new();
    }
    let mut text = String::from_utf8_lossy(&buffer).into_owned();
    if skip_partial {
        if let Some(position) = text.find('\n') {
            text.drain(..=position);
        }
    }
    let mut lines: Vec<String> = text
        .lines()
        .map(|line| line.trim_end_matches('\r').to_string())
        .filter(|line| !line.trim().is_empty())
        .collect();
    if lines.len() > TAIL_LINES {
        lines.drain(..lines.len() - TAIL_LINES);
    }
    lines
}

pub fn log_tail(path: &Path) -> Vec<String> {
    tail(path, TAIL_BYTES)
}

/// Removes pid/socket files left by an agent killed without graceful cleanup.
/// A live pid or a listening socket is never touched.
fn clean_stale(root: &Path, socket: &Path) {
    if !matches!(probe_socket(socket), Ok(SocketProbe::Absent)) || agent_lock_held(root) {
        return;
    }
    if let Some(pid) = read_pid(root) {
        if pid_alive(pid) {
            return;
        }
    }
    let _ = fs::remove_file(root.join("agent.pid"));
    let _ = fs::remove_file(socket);
}

fn monitor_agent(mut child: Child, pid: u32, socket: PathBuf, log: PathBuf, tx: Sender<Msg>) {
    loop {
        if socket_ready(&socket) {
            let _ = tx.send(Msg::Agent(AgentEvent::Started { pid }));
            break;
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let _ = tx.send(Msg::Agent(AgentEvent::StartFailed {
                    pid,
                    code: status.code(),
                    tail: tail(&log, 16 * 1024),
                }));
                return;
            }
            Ok(None) => {}
            Err(err) => {
                let _ = tx.send(Msg::Agent(AgentEvent::StartFailed {
                    pid,
                    code: None,
                    tail: vec![format!("não foi possível acompanhar o processo: {err}")],
                }));
                return;
            }
        }
        thread::sleep(Duration::from_millis(200));
    }
    let code = child.wait().ok().and_then(|status| status.code());
    let _ = tx.send(Msg::Agent(AgentEvent::Exited { pid, code }));
}

/// Builds `wm up ...` arguments from the persisted agent settings.
pub fn args_for_settings(settings: &AgentSettings) -> Result<Vec<String>, String> {
    let mut args = vec![
        "up".to_string(),
        "--udp-port".to_string(),
        settings.udp_port.to_string(),
        "--mtu".to_string(),
        settings.mtu.to_string(),
    ];
    if settings.force_relay {
        args.push("--force-relay".into());
    }
    let tun_name = settings.tun_name.trim();
    if !tun_name.is_empty() {
        args.push("--tun-name".into());
        args.push(tun_name.to_string());
    }
    for spec in settings.expose_specs()? {
        args.push("--expose".into());
        args.push(spec);
    }
    Ok(args)
}

/// Starts a detached agent (or `join` that starts one) and watches its socket.
/// The child handle is moved into the monitor thread, so the process is never
/// killed by dropping a `Child` (Rust does not do that) and is reaped when it
/// exits during this TUI session.
pub fn start(
    root: &Path,
    exe: &Path,
    args: Vec<String>,
    socket: &Path,
    log: &Path,
    tx: &Sender<Msg>,
) -> io::Result<u32> {
    match probe_socket(socket) {
        Ok(SocketProbe::Absent) => {}
        Ok(SocketProbe::Listening) => {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "o agente já está ouvindo no socket"));
        }
        Err(err) => {
            return Err(io::Error::new(err.kind(), format!(
                "não foi possível verificar o socket {}: {err}; nenhum agente novo foi iniciado", socket.display()
            )));
        }
    }
    clean_stale(root, socket);
    rotate_log(log);
    if let Some(parent) = log.parent() {
        fs::create_dir_all(parent)?;
    }
    let out = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(log)?;
    let err = out.try_clone()?;
    let mut command = Command::new(exe);
    command
        .arg("--data-dir")
        .arg(root)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err));
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            libc::signal(libc::SIGHUP, libc::SIG_IGN);
            Ok(())
        });
    }
    let child = command.spawn()?;
    let pid = child.id();
    let _ = tx.send(Msg::Agent(AgentEvent::Starting { pid }));
    let socket = socket.to_path_buf();
    let log = log.to_path_buf();
    let tx = tx.clone();
    thread::spawn(move || monitor_agent(child, pid, socket, log, tx));
    Ok(pid)
}

/// Starts an arbitrary long-running command (`host`, `serve`, `gui`) as a job.
pub fn spawn_job(
    root: &Path,
    exe: &Path,
    label: &str,
    args: &[String],
    log: &Path,
    tx: &Sender<Msg>,
) -> io::Result<u32> {
    rotate_log(log);
    if let Some(parent) = log.parent() {
        fs::create_dir_all(parent)?;
    }
    let out = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(log)?;
    let err = out.try_clone()?;
    let mut command = Command::new(exe);
    command
        .arg("--data-dir")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err));
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            libc::signal(libc::SIGHUP, libc::SIG_IGN);
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    let pid = child.id();
    let label = label.to_string();
    let log_path = log.to_path_buf();
    let _ = tx.send(Msg::Job(JobEvent::Started {
        pid,
        label: label.clone(),
        log: log_path,
    }));
    let tx = tx.clone();
    thread::spawn(move || {
        let code = child.wait().ok().and_then(|status| status.code());
        let _ = tx.send(Msg::Job(JobEvent::Exited { pid, label, code }));
    });
    Ok(pid)
}

/// Sends SIGTERM and watches for the agent socket to disappear.
pub fn stop_agent(root: &Path, socket: &Path, fallback_pid: Option<u32>, tx: &Sender<Msg>) {
    // Prefer the pid this session actually started; the pid file may be stale
    // after a crash and its number could have been reused.
    let pid = fallback_pid
        .filter(|pid| pid_alive(*pid))
        .or_else(|| read_pid(root));
    let Some(pid) = pid else {
        let _ = tx.send(Msg::Agent(AgentEvent::NoAgent));
        return;
    };
    if pid == std::process::id() {
        let _ = tx.send(Msg::Agent(AgentEvent::StopFailed {
            pid,
            error: "o pid do agente aponta para esta TUI".into(),
        }));
        return;
    }
    if !pid_alive(pid) {
        let _ = fs::remove_file(root.join("agent.pid"));
        let _ = tx.send(Msg::Agent(AgentEvent::Stopped { pid }));
        return;
    }
    // Do not signal a recycled pid belonging to some other process: the agent
    // holds the data-dir lock while it runs.
    if !agent_lock_held(root) && !socket_ready(socket) {
        let _ = fs::remove_file(root.join("agent.pid"));
        let _ = tx.send(Msg::Agent(AgentEvent::NoAgent));
        return;
    }
    if let Err(err) = signal(pid, libc::SIGTERM) {
        let _ = tx.send(Msg::Agent(AgentEvent::StopFailed {
            pid,
            error: err.to_string(),
        }));
        return;
    }
    let socket = socket.to_path_buf();
    let tx = tx.clone();
    thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            if !pid_alive(pid) && !socket_ready(&socket) {
                let _ = tx.send(Msg::Agent(AgentEvent::Stopped { pid }));
                return;
            }
            thread::sleep(Duration::from_millis(150));
        }
        let _ = tx.send(Msg::Agent(AgentEvent::StopTimeout { pid }));
    });
}

pub fn stop_job(pid: u32, label: &str, tx: &Sender<Msg>) {
    let _ = signal(pid, libc::SIGTERM);
    let label = label.to_string();
    let tx = tx.clone();
    thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            if !pid_alive(pid) {
                return;
            }
            thread::sleep(Duration::from_millis(200));
        }
        let _ = tx.send(Msg::Job(JobEvent::StopTimeout { pid, label }));
    });
}

/// Runs a bounded CLI command and streams stdout/stderr into the console.
pub fn run_foreground(
    root: &Path,
    exe: &Path,
    args: &[String],
    label: &str,
    tx: &Sender<Msg>,
) -> io::Result<u32> {
    let mut command = Command::new(exe);
    command
        .arg("--data-dir")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let pid = child.id();
    if let Some(stdout) = child.stdout.take() {
        let tx = tx.clone();
        thread::spawn(move || pump(stdout, tx, false));
    }
    if let Some(stderr) = child.stderr.take() {
        let tx = tx.clone();
        thread::spawn(move || pump(stderr, tx, true));
    }
    let tx = tx.clone();
    let label = label.to_string();
    thread::spawn(move || {
        let status = child.wait();
        let code = status.as_ref().ok().and_then(|status| status.code());
        let success = status.as_ref().map(|status| status.success()).unwrap_or(false);
        let _ = tx.send(Msg::CommandFinished {
            pid,
            label,
            code,
            success,
        });
    });
    Ok(pid)
}

fn pump<R: io::Read + Send + 'static>(reader: R, tx: Sender<Msg>, stderr: bool) {
    use io::BufRead;
    let mut reader = io::BufReader::new(reader);
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer) {
            Ok(0) => break,
            Ok(_) => {
                let text = String::from_utf8_lossy(&buffer)
                    .trim_end_matches(['\n', '\r'])
                    .to_string();
                if text.is_empty() {
                    continue;
                }
                let text = if text.len() > 4_000 {
                    let mut cut = 4_000;
                    while !text.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    format!("{}…", &text[..cut])
                } else {
                    text
                };
                if tx
                    .send(Msg::Console(
                        if stderr {
                            crate::app::Level::Warn
                        } else {
                            crate::app::Level::Output
                        },
                        text,
                    ))
                    .is_err()
                {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_root(label: &str) -> PathBuf {
        // Unix socket paths are limited to ~104 bytes; keep the temporary
        // directory short even when TMPDIR is long (macOS /var/folders/...).
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let root = PathBuf::from("/tmp").join(format!(
            "wm-{}-{}-{}",
            &label[..label.len().min(4)],
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn socket_probe_distinguishes_missing_refused_and_listening() {
        let root = temp_root("probe");
        let socket = root.join("agent.sock");
        assert_eq!(probe_socket(&socket).unwrap(), SocketProbe::Absent);
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        assert_eq!(probe_socket(&socket).unwrap(), SocketProbe::Listening);
        drop(listener);
        assert_eq!(probe_socket(&socket).unwrap(), SocketProbe::Absent);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn inaccessible_socket_and_lock_never_trigger_cleanup_or_spawn() {
        use std::os::unix::fs::PermissionsExt;
        // Root bypasses DAC; exercise this test under an ordinary user.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let root = temp_root("denied");
        let socket = root.join("agent.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        fs::write(root.join("agent.pid"), "0\n").unwrap();
        fs::write(root.join("agent.lock"), "").unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o000)).unwrap();
        let probe = probe_socket(&socket);
        let held = agent_lock_held(&root);
        clean_stale(&root, &socket);
        let (tx, _rx) = std::sync::mpsc::channel();
        let started = start(&root, Path::new("/nonexistent/wm"), Vec::new(), &socket, &root.join("agent.log"), &tx);
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(probe.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        assert!(held);
        let error = started.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(error.to_string().contains("nenhum agente novo"));
        assert!(socket.exists());
        assert!(root.join("agent.pid").exists());
        assert!(!root.join("agent.log").exists());
        drop(listener);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unknown_socket_error_preserves_stale_files() {
        let root = temp_root("unknown-probe");
        let socket = root.join("x".repeat(150));
        fs::write(&socket, "not a socket").unwrap();
        fs::write(root.join("agent.pid"), "0\n").unwrap();
        assert!(probe_socket(&socket).is_err());
        clean_stale(&root, &socket);
        assert!(socket.exists());
        assert!(root.join("agent.pid").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pid_liveness_handles_self_bogus_and_stale_file() {
        assert!(pid_alive(std::process::id()));
        assert!(!pid_alive(0));
        assert!(!pid_alive(u32::MAX));
        let root = temp_root("pid");
        assert_eq!(read_pid(&root), None);
        fs::write(root.join("agent.pid"), "not a pid\n").unwrap();
        assert_eq!(read_pid(&root), None);
        fs::write(root.join("agent.pid"), format!("{}\n", std::process::id())).unwrap();
        assert_eq!(read_pid(&root), Some(std::process::id()));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn agent_lock_detects_a_running_process() {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::OpenOptionsExt;
        let root = temp_root("lock");
        assert!(!agent_lock_held(&root));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(root.join("agent.lock"))
            .unwrap();
        let fd = file.as_raw_fd();
        assert_eq!(unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) }, 0);
        assert!(agent_lock_held(&root));
        unsafe {
            libc::flock(fd, libc::LOCK_UN);
        }
        assert!(!agent_lock_held(&root));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tail_reads_last_lines_and_skips_partial_when_truncated() {
        let root = temp_root("tail");
        let log = root.join("agent.log");
        let mut file = File::create(&log).unwrap();
        for index in 0..1000 {
            writeln!(file, "line {index}").unwrap();
        }
        fs::write(root.join("big.log"), "x".repeat(10_000)).unwrap();
        let lines = tail(&log, 300);
        assert_eq!(lines.last().unwrap(), "line 999");
        assert!(lines.len() <= TAIL_LINES);
        assert!(!lines.iter().any(|line| line == "line 0"));
        let empty = tail(&root.join("missing.log"), 10);
        assert!(empty.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn settings_build_safe_argv() {
        let settings = AgentSettings {
            udp_port: 1234,
            mtu: 1300,
            force_relay: true,
            expose: "8421, 9000:9001".into(),
            tun_name: "utun9".into(),
        };
        let args = args_for_settings(&settings).unwrap();
        assert_eq!(
            args,
            vec![
                "up",
                "--udp-port",
                "1234",
                "--mtu",
                "1300",
                "--force-relay",
                "--tun-name",
                "utun9",
                "--expose",
                "8421",
                "--expose",
                "9000:9001"
            ]
        );
        let broken = AgentSettings {
            expose: "not-a-port".into(),
            ..AgentSettings::default()
        };
        assert!(args_for_settings(&broken).is_err());
    }

    #[test]
    fn detached_agent_starts_in_own_session_and_stops_on_signal() {
        use std::os::unix::fs::PermissionsExt;
        let python_ok = std::process::Command::new("python3")
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false);
        if !python_ok {
            eprintln!("python3 indisponível; pulando teste do agente destacado");
            return;
        }
        struct KillOnDrop(Option<u32>);
        impl Drop for KillOnDrop {
            fn drop(&mut self) {
                if let Some(pid) = self.0 {
                    if pid_alive(pid) {
                        let _ = signal(pid, libc::SIGKILL);
                    }
                }
            }
        }
        let root = temp_root("detached");
        let short = format!(
            "/tmp/wm-tui-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let socket = PathBuf::from(&short);
        let log = root.join("agent.log");
        let script = root.join("fake-agent.py");
        let body = format!(
            "#!/usr/bin/env python3\nimport os, socket, threading, time\nprint('fake agent up', flush=True)\npath = '{short}'\ntry:\n    os.unlink(path)\nexcept FileNotFoundError:\n    pass\ns = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)\ns.bind(path)\ns.listen(64)\nprint('fake agent listening', flush=True)\ndef drain():\n    while True:\n        try:\n            c, _ = s.accept()\n            c.close()\n        except OSError:\n            return\nthreading.Thread(target=drain, daemon=True).start()\nwhile True:\n    time.sleep(0.2)\n"
        );
        fs::write(&script, body).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

        let (tx, rx) = std::sync::mpsc::channel();
        let pid = start(
            &root,
            &script,
            vec!["up".into()],
            &socket,
            &log,
            &tx,
        )
        .expect("start detached fake agent");
        let mut guard = KillOnDrop(Some(pid));

        let started = wait_for_agent(&rx, 15_000).expect("agent should announce itself");
        assert_eq!(started, pid);
        let deadline = Instant::now() + Duration::from_secs(3);
        while !socket_ready(&socket) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
        assert!(socket_ready(&socket));
        let session = unsafe { libc::getsid(pid as libc::pid_t) };
        assert_eq!(session, pid as libc::pid_t, "setsid deve isolar o agente");
        let text = fs::read_to_string(&log).unwrap_or_default();
        assert!(text.contains("fake agent up"), "log: {text:?}");
        assert!(root.join("agent.log").exists());

        stop_agent(&root, &socket, Some(pid), &tx);
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if !pid_alive(pid) {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        assert!(!pid_alive(pid), "fake agent should be gone");
        guard.0 = None;
        let _ = fs::remove_file(&socket);
        fs::remove_dir_all(root).unwrap();
    }

    fn wait_for_agent(rx: &std::sync::mpsc::Receiver<Msg>, timeout_ms: u64) -> Option<u32> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        while Instant::now() < deadline {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left.min(Duration::from_millis(200))) {
                Ok(Msg::Agent(AgentEvent::Started { pid })) => return Some(pid),
                Ok(_) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return None,
            }
        }
        None
    }

    #[test]
    fn log_is_rotated_only_when_large() {
        let root = temp_root("rotate");
        let log = root.join("agent.log");
        fs::write(&log, "small").unwrap();
        rotate_log(&log);
        assert!(log.exists());
        let huge = OpenOptions::new().append(true).open(&log).unwrap();
        huge.set_len(LOG_ROTATE_BYTES + 1).unwrap();
        rotate_log(&log);
        assert!(!log.exists());
        assert!(root.join("agent.log.1").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
