//! macOS desktop bridge. Administrator authorization explicitly delegates one
//! root-owned node directory to one UID. No arbitrary command proxy is exposed.
use std::path::{Path, PathBuf};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::process::{Command, Stdio};
use anyhow::{Context, bail, ensure};
use serde::{Serialize, Deserialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use wm_proto::{LocalRequest, LocalResponse};

const INSTALL: &str = "/Library/PrivilegedHelperTools/wired-magic-desktop";
const HELPER: &str = "/Library/PrivilegedHelperTools/wired-magic-desktop/wm";
const CONFIG: &str = "/Library/PrivilegedHelperTools/wired-magic-desktop/config.json";

#[derive(clap::Args)]
pub struct DesktopArgs {
    #[command(subcommand)]
    action: Action,
}
#[derive(clap::Subcommand)]
enum Action {
    /// Explicit administrator-authorized installation; never called silently.
    Install { #[arg(long)] root: PathBuf, #[arg(long)] uid: u32 },
    Activate,
    Broker,
    Daemon,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config { root: PathBuf, uid: u32 }

fn runtime(uid: u32) -> PathBuf { PathBuf::from(format!("/Library/Application Support/wired-magic-desktop-{uid}")) }

/// Mode bits do not describe inherited macOS ACL grants. Reject every extended
/// ACL (including deny-only ACLs) rather than trying to normalize admin policy.
#[cfg(target_os = "macos")]
fn no_extended_acl(path: &Path) -> anyhow::Result<()> {
    use std::ffi::{CString, c_void, c_int, c_char};
    unsafe extern "C" {
        fn acl_get_file(path: *const c_char, kind: c_int) -> *mut c_void;
        fn acl_get_entry(acl: *mut c_void, entry: c_int, out: *mut *mut c_void) -> c_int;
        fn acl_valid(acl: *mut c_void) -> c_int;
        fn acl_free(acl: *mut c_void) -> c_int;
    }
    let path_c = CString::new(path.as_os_str().as_encoded_bytes())?;
    let acl = unsafe { acl_get_file(path_c.as_ptr(), 0x100) };
    if acl.is_null() {
        let error = std::io::Error::last_os_error();
        // Darwin reports ENOENT for an existing object with no extended ACL.
        // Verify existence separately; missing objects and all other errors fail closed.
        if error.raw_os_error() == Some(libc::ENOENT) {
            std::fs::symlink_metadata(path)?;
            return Ok(());
        }
        return Err(error).with_context(|| format!("não foi possível verificar ACL de {}", path.display()));
    }
    let mut entry = std::ptr::null_mut();
    let valid = unsafe { acl_valid(acl) } == 0;
    let result = unsafe { acl_get_entry(acl, 0, &mut entry) };
    let errno = std::io::Error::last_os_error().raw_os_error();
    unsafe { acl_free(acl); }
    ensure!(valid && result == -1 && errno == Some(libc::EINVAL),
        "ACL estendida ou não verificável em {}; requer revisão administrativa", path.display());
    Ok(())
}
#[cfg(not(target_os = "macos"))]
fn no_extended_acl(_path: &Path) -> anyhow::Result<()> { Ok(()) }

/// Reject symlinks and writable/untrusted ancestors, including the leaf.
fn trusted(path: &Path) -> anyhow::Result<()> {
    ensure!(path.is_absolute(), "caminho root deve ser absoluto");
    ensure!(!path.components().any(|c| matches!(c, std::path::Component::ParentDir)), "caminho não normalizado");
    let mut p = PathBuf::from("/");
    for part in path.components().skip(1) {
        p.push(part);
        let m = std::fs::symlink_metadata(&p).with_context(|| format!("validando {}", p.display()))?;
        ensure!(!m.file_type().is_symlink() && m.uid() == 0 && m.mode() & 0o022 == 0,
            "{} precisa pertencer a root, sem symlink ou escrita de grupo/outros", p.display());
        no_extended_acl(&p)?;
    }
    Ok(())
}
fn validate_delegated_groups(groups: &str) -> anyhow::Result<()> {
    let groups = groups.split_whitespace().map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>()?;
    ensure!(!groups.is_empty() && !groups.contains(&1),
        "UID delegado não pode pertencer ao grupo sistema daemon");
    Ok(())
}
fn validate_delegated_uid(uid: u32) -> anyhow::Result<()> {
    ensure!(uid > 0, "UID autorizado não pode ser root");
    let output = Command::new("/usr/bin/id").args(["-G", &uid.to_string()])
        .env_clear().stdin(Stdio::null()).output()?;
    ensure!(output.status.success(), "não foi possível verificar grupos do UID delegado");
    validate_delegated_groups(std::str::from_utf8(&output.stdout)?)
}

/// Compatibility only: macOS /private/var/run is root:daemon 0775.
/// Trust that system group here, never for helper code, node state or IPC.
/// The delegated UI UID is explicitly forbidden from belonging to daemon.
fn machine_lock() -> anyhow::Result<crate::service::AgentLock> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    ensure!(unsafe { libc::geteuid() } == 0, "machine lock exige root");
    trusted(Path::new("/private/var"))?;
    let path = Path::new("/private/var/run");
    no_extended_acl(path)?;
    let parent = std::fs::OpenOptions::new().read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW).open(path)?;
    let meta = parent.metadata()?;
    ensure!(meta.is_dir() && meta.uid() == 0 && meta.mode() & 0o002 == 0
        && (meta.mode() & 0o020 == 0 || meta.gid() == 1), "runtime sistema não confiável");
    let name = CString::new("wired-magic")?;
    if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o755) } != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EEXIST) { return Err(error.into()); }
    }
    let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    if fd < 0 { return Err(std::io::Error::last_os_error().into()); }
    let dir = unsafe { std::fs::File::from_raw_fd(fd) };
    let meta = dir.metadata()?;
    ensure!(meta.is_dir() && meta.uid() == 0 && meta.mode() & 0o022 == 0, "diretório machine lock não confiável");
    no_extended_acl(&path.join("wired-magic"))?;
    let name = CString::new("agent.lock")?;
    let fd = unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), libc::O_RDONLY | libc::O_CREAT | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK, 0o600) };
    if fd < 0 { return Err(std::io::Error::last_os_error().into()); }
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    let meta = file.metadata()?;
    ensure!(meta.is_file() && meta.uid() == 0 && meta.mode() & 0o022 == 0 && meta.nlink() == 1,
        "arquivo machine lock não confiável");
    no_extended_acl(&path.join("wired-magic/agent.lock"))?;
    crate::service::AgentLock::from_verified_file(file)
}
fn root_config() -> anyhow::Result<Config> {
    ensure!(unsafe { libc::geteuid() } == 0, "helper exige autorização de administrador");
    trusted(Path::new(HELPER))?;
    trusted(Path::new(CONFIG))?;
    let cfg: Config = serde_json::from_slice(&std::fs::read(CONFIG)?)?;
    validate_delegated_uid(cfg.uid)?;
    trusted(&cfg.root)?;
    let root_meta = std::fs::metadata(&cfg.root)?;
    ensure!(root_meta.is_dir() && root_meta.mode() & 0o077 == 0, "root deve ser diretório privado 0700");
    Ok(cfg)
}
fn safe_existing_file(path: &Path) -> anyhow::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            trusted(path)?;
            ensure!(meta.is_file() && meta.nlink() == 1, "arquivo root não regular ou com hardlink: {}", path.display());
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    Ok(())
}
fn detached(args: &[&str], log: &Path) -> anyhow::Result<std::process::Child> {
    use std::os::unix::process::CommandExt;
    safe_existing_file(&std::fs::canonicalize(log.parent().context("log sem diretório")?)?.join(log.file_name().context("log sem nome")?))?;
    let log = std::fs::OpenOptions::new().create(true).append(true).mode(0o600)
        .custom_flags(libc::O_NOFOLLOW).open(log)?;
    ensure!(log.metadata()?.uid() == 0 && log.metadata()?.mode() & 0o077 == 0, "log deve ser root privado 0600");
    let mut cmd = Command::new(HELPER);
    cmd.args(args).env_clear().env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .current_dir("/").stdin(Stdio::null()).stdout(log.try_clone()?).stderr(log);
    unsafe { cmd.pre_exec(|| { if libc::setsid() < 0 { return Err(std::io::Error::last_os_error()); } Ok(()) }); }
    Ok(cmd.spawn()?)
}
pub async fn run(args: DesktopArgs) -> anyhow::Result<()> {
    ensure!(cfg!(target_os = "macos"), "desktop helper disponível somente no macOS");
    match args.action {
        Action::Install { root, uid } => {
            ensure!(unsafe { libc::geteuid() } == 0 && uid > 0, "instalação exige admin e UID não-root");
            validate_delegated_uid(uid)?;
            let root = std::fs::canonicalize(root)?;
            trusted(&root)?;
            ensure!(std::fs::metadata(&root)?.is_dir(), "root deve existir e pertencer a root");
            trusted(Path::new("/Library/PrivilegedHelperTools"))?;
            if !Path::new(INSTALL).exists() { std::fs::create_dir(INSTALL)?; }
            trusted(Path::new(INSTALL))?;
            if Path::new(CONFIG).exists() {
                let existing = root_config()?;
                ensure!(existing.root == root && existing.uid == uid,
                    "helper já autorizado para outro root/UID; reconfiguração exige remoção administrativa explícita, não substituição silenciosa");
            }
            std::fs::set_permissions(INSTALL, std::fs::Permissions::from_mode(0o700))?;
            // New files only: never truncate a symlink or overwrite a live executable.
            let tmp = Path::new(INSTALL).join(format!("wm-install-{}", std::process::id()));
            let mut dest = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o700).open(&tmp)?;
            let mut source = std::fs::File::open(std::env::current_exe()?)?;
            std::io::copy(&mut source, &mut dest)?;
            dest.sync_all()?;
            std::fs::rename(&tmp, HELPER)?;
            let tmp_cfg = Path::new(INSTALL).join(format!("config-install-{}", std::process::id()));
            let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp_cfg)?;
            use std::io::Write;
            file.write_all(&serde_json::to_vec(&Config { root, uid })?)?;
            file.sync_all()?;
            std::fs::rename(tmp_cfg, CONFIG)?;
            let cfg = root_config()?;
            activate(&cfg).await
        }
        Action::Activate => { let cfg = root_config()?; activate(&cfg).await }
        Action::Broker => { let cfg = root_config()?; broker(cfg).await }
        Action::Daemon => {
            let cfg = root_config()?;
            ensure!(node(&cfg)?.is_some(), "nó não inscrito");
            for name in ["agent.lock", "agent.pid", "agent-state.json.tmp"] {
                safe_existing_file(&cfg.root.join(name))?;
            }
            let _instance = (machine_lock()?, crate::service::AgentLock::acquire(&cfg.root)?);
            let mut agent = wm_core::AgentConfig::with_data_dir(cfg.root);
            agent.disable_extensions = true;
            wm_core::run(agent).await
        }
    }
}
async fn activate(cfg: &Config) -> anyhow::Result<()> {
    let dir = runtime(cfg.uid);
    if !dir.exists() { std::fs::create_dir(&dir)?; }
    trusted(&dir)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755))?;
    let socket = dir.join("agent.sock");
    match UnixStream::connect(&socket).await {
        Ok(stream) => { ensure!(stream.peer_cred()?.uid() == 0, "broker peer não-root"); return Ok(()); }
        Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused) => {}
        Err(e) => return Err(e.into()),
    }
    detached(&["desktop-helper", "broker"], &dir.join("broker.log"))?;
    // Wait only in the authorization worker, never in the UI thread.
    for _ in 0..50 {
        if let Ok(s) = UnixStream::connect(&socket).await { ensure!(s.peer_cred()?.uid() == 0, "broker peer inválido"); return Ok(()); }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    bail!("broker não iniciou; veja log root privado")
}
fn allowed(method: &str) -> bool {
    matches!(method, "status" | "peers" | "shares" | "share_resolve" | "share_create" |
        "share_update" | "share_delete" | "rename_node" | "set_profile" | "resolve" |
        "ping" | "reconnect" | "metrics" | "shutdown")
}
fn node(cfg: &Config) -> anyhow::Result<Option<wm_core::NodeState>> {
    let path = cfg.root.join("agent-state.json");
    match std::fs::symlink_metadata(&path) {
        Ok(meta) => {
            trusted(&path)?;
            ensure!(meta.is_file() && meta.mode() & 0o077 == 0, "agent-state exige arquivo root privado 0600");
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    wm_core::NodeState::load(&path)
}
fn public_metadata(n: wm_core::NodeState) -> Value {
    json!({"node_name": n.node_name, "overlay_ip": n.overlay_ip,
        "overlay_cidr": n.overlay_cidr,
        "control_url": reqwest::Url::parse(&n.control_url).map(|url| url.origin().ascii_serialization()).unwrap_or_default(),
        "network_name": n.network_name, "created_at": n.created_at,
        "display_name": n.display_name, "about": n.about})
}
async fn forward(cfg: &Config, req: &LocalRequest) -> anyhow::Result<Value> {
    use std::os::unix::fs::FileTypeExt;
    let meta = std::fs::symlink_metadata(cfg.root.join("agent.sock"))?;
    ensure!(meta.file_type().is_socket() && meta.uid() == 0 && meta.gid() == 0 && meta.mode() & 0o007 == 0, "socket privado inválido");
    let stream = UnixStream::connect(cfg.root.join("agent.sock")).await?;
    ensure!(stream.peer_cred()?.uid() == 0, "daemon peer não-root");
    let mut stream = BufReader::new(stream);
    let mut data = serde_json::to_vec(req)?; data.push(b'\n');
    stream.get_mut().write_all(&data).await?;
    use tokio::io::AsyncReadExt;
    let mut response = Vec::new();
    (&mut stream).take(4 * 1024 * 1024 + 1).read_until(b'\n', &mut response).await?;
    ensure!(response.len() <= 4 * 1024 * 1024 && response.last() == Some(&b'\n'), "resposta excessiva ou incompleta");
    let response: LocalResponse = serde_json::from_slice(&response)?;
    ensure!(response.ok, "{}", response.error.unwrap_or_default());
    Ok(response.result.unwrap_or(Value::Null))
}
async fn broker(cfg: Config) -> anyhow::Result<()> {
    use std::os::fd::AsRawFd;
    let dir = runtime(cfg.uid);
    trusted(&dir)?;
    // Lifetime flock serializes brokers and prevents unlinking a live socket.
    let lock = std::fs::OpenOptions::new().create(true).write(true).mode(0o600)
        .custom_flags(libc::O_NOFOLLOW).open(dir.join("broker.lock"))?;
    ensure!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0, "broker já ativo");
    let path = dir.join("agent.sock");
    match std::fs::symlink_metadata(&path) {
        Ok(m) => {
            use std::os::unix::fs::FileTypeExt;
            ensure!(m.uid() == cfg.uid && m.file_type().is_socket(), "socket existente inválido");
            std::fs::remove_file(&path)?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())?;
    ensure!(unsafe { libc::chown(c.as_ptr(), cfg.uid, 0) } == 0, "chown do socket falhou");
    // Root owns directory; UID owns only the socket, never the directory/config.
    let cfg = std::sync::Arc::new(cfg);
    let child = std::sync::Arc::new(tokio::sync::Mutex::new(None::<std::process::Child>));
    let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(16));
    loop {
        let (stream, _) = listener.accept().await?;
        // A client may disconnect before credentials can be read. Treat that
        // connection as unauthenticated, never as a fatal broker error.
        match stream.peer_cred() {
            Ok(peer) if peer.uid() == cfg.uid => {}
            _ => continue,
        }
        let Ok(permit) = permits.clone().try_acquire_owned() else { continue; };
        let cfg = cfg.clone(); let child = child.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let _ = tokio::time::timeout(std::time::Duration::from_secs(15), async {
                let mut stream = BufReader::new(stream);
                let mut line = Vec::new();
                // Bounded line reader, exactly one request per connection.
                use tokio::io::AsyncReadExt;
                (&mut stream).take(65537).read_until(b'\n', &mut line).await?;
                ensure!(line.len() <= 65536 && line.last() == Some(&b'\n'), "requisição excessiva");
                let req: LocalRequest = serde_json::from_slice(&line)?;
                let result: anyhow::Result<Value> = async {
                    if req.method == "desktop_identity" {
                        return Ok(json!({"root": cfg.root, "uid": cfg.uid}));
                    }
                    if req.method == "desktop_metadata" {
                        let node = node(&cfg)?;
                        return Ok(node.map(public_metadata).unwrap_or(Value::Null));
                    }
                    if req.method == "desktop_start" {
                        if let Ok(meta) = std::fs::symlink_metadata(cfg.root.join("agent.sock")) {
                            use std::os::unix::fs::FileTypeExt;
                            ensure!(meta.file_type().is_socket() && meta.uid() == 0 && meta.gid() == 0 && meta.mode() & 0o007 == 0,
                                "socket privado existente inválido; nenhum agente iniciado");
                        }
                        let mut child = child.lock().await;
                        match UnixStream::connect(cfg.root.join("agent.sock")).await {
                            Ok(s) => { ensure!(s.peer_cred()?.uid() == 0, "daemon peer inválido"); return Ok(json!({"attached": true})); }
                            Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused) => {}
                            Err(e) => return Err(e.into()),
                        }
                        if let Some(process) = child.as_mut() { if process.try_wait()?.is_none() { return Ok(json!({"starting": true})); } }
                        ensure!(node(&cfg)?.is_some(), "nó não inscrito; enrollment root ainda não suportado no desktop");
                        *child = Some(detached(&["desktop-helper", "daemon"], &cfg.root.join("desktop-agent.log"))?);
                        return Ok(json!({"starting": true}));
                    }
                    ensure!(allowed(&req.method), "operação não autorizada pelo bridge desktop");
                    forward(&cfg, &req).await
                }.await;
                let response = match result { Ok(v) => LocalResponse::ok(req.id, v), Err(e) => LocalResponse::err(req.id, format!("{e:#}")) };
                let mut out = serde_json::to_vec(&response)?; out.push(b'\n');
                stream.get_mut().write_all(&out).await?;
                Ok::<(), anyhow::Error>(())
            }).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "macos")]
    #[test] fn acl_absence_is_not_a_missing_object() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plain");
        std::fs::write(&path, b"test").unwrap();
        no_extended_acl(&path).unwrap();
        assert!(no_extended_acl(&dir.path().join("missing")).is_err());
    }
    #[cfg(target_os = "macos")]
    #[test] fn extended_acl_is_still_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("with-acl");
        std::fs::write(&path, b"test").unwrap();
        let status = Command::new("/bin/chmod")
            .args(["+a", "everyone allow read"]).arg(&path).status().unwrap();
        assert!(status.success());
        assert!(no_extended_acl(&path).is_err());
    }
    #[test] fn system_daemon_group_cannot_be_delegated() {
        validate_delegated_groups("20 12 80").unwrap();
        for denied in ["1", "20 1 80", "", "20 unknown"] {
            assert!(validate_delegated_groups(denied).is_err());
        }
    }
    #[test] fn desktop_api_is_not_an_execution_proxy() {
        for denied in ["extension", "extension_activation", "extension_catalog", "probe", "logs", "exec", "join", "up", "desktop_install"] { assert!(!allowed(denied), "{denied}"); }
        assert!(allowed("status")); assert!(allowed("shutdown"));
    }
    #[test] fn metadata_never_serializes_keys_tokens_or_url_credentials() {
        let state: wm_core::NodeState = serde_json::from_value(json!({
            "node_id": "00000000-0000-0000-0000-000000000001", "node_token": "SECRET_TOKEN",
            "wg_private_key": "SECRET_KEY", "wg_public_key": "public", "overlay_ip": "10.0.0.1",
            "control_url": "https://alice:SECRET_PASSWORD@example.test/base?token=SECRET_QUERY#SECRET_FRAGMENT",
            "node_name": "node", "network_name": "network", "created_at": "2026-01-01T00:00:00Z"
        })).unwrap();
        let metadata = public_metadata(state);
        assert_eq!(metadata["control_url"], "https://example.test");
        assert!(!metadata.to_string().contains("SECRET"));
        assert!(metadata.get("node_token").is_none());
        assert!(metadata.get("wg_private_key").is_none());
    }
    #[test] fn config_does_not_accept_arbitrary_command_fields() {
        assert!(serde_json::from_value::<Config>(json!({"root": "/root", "uid": 501, "command": "/bin/sh"})).is_err());
    }
    #[test] fn rejects_relative_and_user_owned_roots() {
        assert!(trusted(Path::new("relative")).is_err());
        let d = tempfile::tempdir().unwrap();
        if unsafe { libc::geteuid() } != 0 { assert!(trusted(d.path()).is_err()); }
    }
}
