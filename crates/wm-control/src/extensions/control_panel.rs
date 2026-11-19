//! Optional, server-only React panel. No frontend is compiled into the server.
//! Packages are trusted same-origin code, not a sandbox. Activation pins a fully
//! verified in-memory snapshot, never a URL-controlled filesystem path.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use anyhow::{Context, ensure};
use async_trait::async_trait;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::get;
use axum::{Json, Router};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use wm_ext::ExtensionManifest;
use wm_proto::ExtensionDesired;

use super::ControlExtension;
use crate::api::{AdminAuth, ApiErr};
use crate::db::{self, NodeRow};
use crate::state::AppState;

pub const ID: &str = "control_panel";
const MANIFEST: &str = include_str!("../../../../extensions/public/control_panel/extension.toml");
const MAX_FILES: usize = 1024;
const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;
const MAX_TOTAL_BYTES: usize = 32 * 1024 * 1024;
const MAX_MANIFEST_BYTES: usize = 512 * 1024;

pub struct PanelExtension;

#[async_trait]
impl ControlExtension for PanelExtension {
    fn id(&self) -> &'static str { ID }
    fn manifest(&self) -> ExtensionManifest {
        toml::from_str(MANIFEST).expect("valid built-in control_panel metadata")
    }
    fn router(&self) -> Router<Arc<AppState>> {
        Router::new().route("/extensions/control_panel/runtime", get(status).put(set_runtime))
    }
    async fn desired_for(&self, _state: &AppState, _node: &NodeRow) -> anyhow::Result<Option<ExtensionDesired>> {
        // Server-only: no hooks, processes, node activation, or wire changes.
        Ok(None)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleManifest {
    schema: u32,
    id: String,
    version: String,
    files: BTreeMap<String, String>,
}

#[derive(Clone)]
pub(crate) struct PanelAsset {
    pub bytes: Bytes,
    pub mime: &'static str,
}

pub(crate) struct PanelBundle {
    pub digest: String,
    files: BTreeMap<String, PanelAsset>,
    total_bytes: usize,
}

impl PanelBundle {
    pub fn asset(&self, requested: &str) -> Option<(&str, PanelAsset)> {
        if requested.is_empty() {
            return self.files.get_key_value("index.html").map(|(name, asset)| (name.as_str(), asset.clone()));
        }
        if !safe_relative_path(requested) { return None; }
        if let Some((name, asset)) = self.files.get_key_value(requested) {
            return Some((name.as_str(), asset.clone()));
        }
        // A missing static file must never become an HTML response.
        if requested == "assets" || requested.starts_with("assets/")
            || requested.rsplit('/').next().is_some_and(|name| name.contains('.')) {
            return None;
        }
        self.files.get_key_value("index.html").map(|(name, asset)| (name.as_str(), asset.clone()))
    }
}

pub struct PanelRuntime {
    data_dir: PathBuf,
    active: RwLock<Option<Arc<PanelBundle>>>,
    // Serialize persistence + snapshot changes; concurrent toggles cannot leave
    // disk enabled and memory disabled (or install a stale inspected bundle).
    mutation: Mutex<()>,
}

impl PanelRuntime {
    pub fn load(data_dir: &Path) -> Self {
        let runtime = Self {
            data_dir: data_dir.to_path_buf(),
            active: RwLock::new(None),
            mutation: Mutex::new(()),
        };
        let restored = (|| -> anyhow::Result<Option<PanelBundle>> {
            let dir = trusted_data_dir(data_dir)?;
            let trust = match open_relative(&dir, "control-panel-trust.sha256", false) {
                Ok(file) => read_bounded(file, 64)?,
                Err(error) if error.downcast_ref::<std::io::Error>().is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) => return Ok(None),
                Err(error) => return Err(error),
            };
            let expected = std::str::from_utf8(&trust)?;
            ensure!(valid_digest(expected), "invalid persisted panel trust");
            let bundle = load_bundle(data_dir)?;
            ensure!(bundle.digest == expected, "panel package changed; explicit activation required");
            Ok(Some(bundle))
        })();
        match restored {
            Ok(Some(bundle)) => *runtime.active.write().expect("new panel lock") = Some(Arc::new(bundle)),
            Ok(None) => {},
            Err(error) => tracing::warn!(%error, "optional control panel remains disabled"),
        }
        runtime
    }

    pub(crate) fn snapshot(&self) -> Option<Arc<PanelBundle>> {
        self.active.read().ok()?.clone()
    }

    fn inspect(&self) -> PanelStatus {
        let active_digest = self.snapshot().map(|bundle| bundle.digest.clone());
        match load_bundle(&self.data_dir) {
            Ok(bundle) => PanelStatus {
                id: ID.to_string(), enabled: active_digest.is_some(), installed: true,
                digest: Some(bundle.digest), active_digest, files: bundle.files.len(),
                bytes: bundle.total_bytes, error: None,
            },
            Err(error) => PanelStatus {
                id: ID.to_string(), enabled: active_digest.is_some(), installed: false,
                digest: None, active_digest, files: 0, bytes: 0,
                error: Some(format!("{error:#}")),
            },
        }
    }

    fn set(&self, input: RuntimeInput) -> anyhow::Result<PanelStatus> {
        let _mutation = self.mutation.lock().map_err(|_| anyhow::anyhow!("panel mutation lock poisoned"))?;
        if input.enabled {
            let expected = input.digest.as_deref().ok_or_else(|| anyhow::anyhow!("digest required to activate trusted panel package"))?;
            ensure!(valid_digest(expected), "invalid digest");
            let bundle = load_bundle(&self.data_dir)?;
            ensure!(bundle.digest == expected, "package changed; inspect and confirm its digest again");
            persist_trust(&self.data_dir, expected)?;
            *self.active.write().map_err(|_| anyhow::anyhow!("panel lock poisoned"))? = Some(Arc::new(bundle));
        } else {
            // Disable serving even if removing a damaged trust file fails.
            *self.active.write().map_err(|_| anyhow::anyhow!("panel lock poisoned"))? = None;
            let path = self.data_dir.join("control-panel-trust.sha256");
            match fs::remove_file(path) {
                Ok(()) => {},
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
                Err(error) => return Err(error.into()),
            }
        }
        Ok(self.inspect())
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PanelStatus {
    pub id: String,
    pub enabled: bool,
    pub installed: bool,
    pub digest: Option<String>,
    pub active_digest: Option<String>,
    pub files: usize,
    pub bytes: usize,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeInput {
    enabled: bool,
    digest: Option<String>,
}

async fn status(State(state): State<Arc<AppState>>, _auth: AdminAuth) -> Result<Json<PanelStatus>, ApiErr> {
    if db::setup_required(&state.db).await? {
        return Err(ApiErr::forbidden("setup_required", "conclua o bootstrap antes de administrar o painel"));
    }
    let runtime = state.extensions.panel.clone();
    let result = tokio::task::spawn_blocking(move || runtime.inspect()).await
        .map_err(|_| ApiErr::new(axum::http::StatusCode::INTERNAL_SERVER_ERROR, "panel_task_failed", "falha ao inspecionar painel"))?;
    Ok(Json(result))
}

async fn set_runtime(
    State(state): State<Arc<AppState>>, auth: AdminAuth, headers: HeaderMap, Json(input): Json<RuntimeInput>,
) -> Result<Json<PanelStatus>, ApiErr> {
    auth.require_csrf(&headers)?;
    if db::setup_required(&state.db).await? {
        return Err(ApiErr::forbidden("setup_required", "conclua o bootstrap antes de ativar o painel"));
    }
    let runtime = state.extensions.panel.clone();
    let result = tokio::task::spawn_blocking(move || runtime.set(input)).await
        .map_err(|_| ApiErr::new(axum::http::StatusCode::INTERNAL_SERVER_ERROR, "panel_task_failed", "falha ao alterar painel"))?
        .map_err(|error| ApiErr::bad_request("panel_package_invalid", format!("{error:#}")))?;
    db::audit(&state.db, &format!("admin:{}", auth.username()), "extension.control_panel.runtime", ID,
        Some(if result.enabled { "enabled" } else { "disabled" }), crate::api::client_ip(&headers).as_deref()).await.ok();
    Ok(Json(result))
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(crate) fn safe_relative_path(path: &str) -> bool {
    !path.is_empty() && path.len() <= 240 && path.split('/').count() <= 12
        && path.split('/').all(|part| !part.is_empty() && !part.starts_with('.')
            && part.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)))
}

fn mime(path: &str) -> Option<&'static str> {
    if path == "index.html" { return Some("text/html; charset=utf-8"); }
    Some(match path.rsplit('.').next()? {
        "js" | "mjs" => "text/javascript; charset=utf-8", "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml", "png" => "image/png", "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif", "webp" => "image/webp", "avif" => "image/avif", "ico" => "image/x-icon",
        "woff" => "font/woff", "woff2" => "font/woff2", "ttf" => "font/ttf", "otf" => "font/otf",
        _ => return None,
    })
}

fn read_bounded(file: File, limit: usize) -> anyhow::Result<Vec<u8>> {
    ensure!(file.metadata()?.is_file(), "panel asset is not a regular file");
    ensure!(file.metadata()?.len() <= limit as u64, "panel file exceeds byte limit");
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= limit, "panel file grew beyond byte limit");
    Ok(bytes)
}

fn trusted_data_dir(data_dir: &Path) -> anyhow::Result<File> {
    let canonical = data_dir.canonicalize().context("canonical control data directory")?;
    let file = OpenOptions::new().read(true).open(canonical)?;
    ensure!(file.metadata()?.is_dir(), "control data directory required");
    Ok(file)
}

/// Walk relative to held directory descriptors, never following symlinks.
/// On unsupported platforms loading fails closed rather than weakening this.
#[cfg(unix)]
fn open_relative(root: &File, path: &str, directory: bool) -> anyhow::Result<File> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    ensure!(safe_relative_path(path), "invalid relative panel path");
    let mut current = root.try_clone()?;
    let parts: Vec<_> = path.split('/').collect();
    for (index, part) in parts.iter().enumerate() {
        let name = CString::new(*part)?;
        let is_dir = directory || index + 1 < parts.len();
        let flags = libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK
            | if is_dir { libc::O_DIRECTORY } else { 0 };
        // SAFETY: current holds a live directory fd and name is NUL-terminated.
        // O_NOFOLLOW applies to every component; the fd is owned exactly once.
        let fd = unsafe { libc::openat(current.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 { return Err(std::io::Error::last_os_error().into()); }
        current = unsafe { File::from_raw_fd(fd) };
        if is_dir { ensure!(current.metadata()?.is_dir(), "panel directory required"); }
    }
    Ok(current)
}

#[cfg(not(unix))]
fn open_relative(_root: &File, _path: &str, _directory: bool) -> anyhow::Result<File> {
    anyhow::bail!("external panel assets require Unix no-follow directory handles")
}

fn load_bundle(data_dir: &Path) -> anyhow::Result<PanelBundle> {
    let root = trusted_data_dir(data_dir)?;
    let www = open_relative(&root, "extensions/control_panel/www", true).context("panel package not installed")?;
    let bytes = read_bounded(open_relative(&www, "bundle.json", false)?, MAX_MANIFEST_BYTES)?;
    let manifest: BundleManifest = serde_json::from_slice(&bytes).context("invalid panel bundle manifest")?;
    ensure!(manifest.schema == 1 && manifest.id == ID, "unsupported panel bundle schema/id");
    ensure!(!manifest.version.is_empty() && manifest.version.len() <= 64, "invalid panel version");
    ensure!(!manifest.files.is_empty() && manifest.files.len() <= MAX_FILES, "panel file count limit");
    ensure!(manifest.files.contains_key("index.html"), "index.html is required");
    let mut files = BTreeMap::new();
    let mut total_bytes = 0;
    for (name, expected) in manifest.files {
        ensure!(safe_relative_path(&name) && valid_digest(&expected), "invalid bundle path/hash");
        let mime = mime(&name).ok_or_else(|| anyhow::anyhow!("unsupported panel asset type: {name}"))?;
        let data = read_bounded(open_relative(&www, &name, false)?, MAX_FILE_BYTES)?;
        total_bytes += data.len();
        ensure!(total_bytes <= MAX_TOTAL_BYTES, "panel total byte limit");
        ensure!(format!("{:x}", Sha256::digest(&data)) == expected, "panel asset hash mismatch: {name}");
        files.insert(name, PanelAsset { bytes: Bytes::from(data), mime });
    }
    Ok(PanelBundle { digest: format!("{:x}", Sha256::digest(&bytes)), files, total_bytes })
}

fn persist_trust(data_dir: &Path, digest: &str) -> anyhow::Result<()> {
    // Only the trusted control data directory can contain this marker. It is
    // deliberately outside the installable package, so unpacking never enables.
    let dir = data_dir.canonicalize()?;
    let temporary = dir.join(format!(".control-panel-trust-{}", uuid::Uuid::new_v4()));
    let destination = dir.join("control-panel-trust.sha256");
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> anyhow::Result<()> {
        let mut file = options.open(&temporary)?;
        file.write_all(digest.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, destination)?;
        File::open(&dir)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() { let _ = fs::remove_file(&temporary); }
    result
}

#[cfg(test)]
mod tests;
