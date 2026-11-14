//! Extension package manager: git sources and flake-style repositories.
//!
//! A package source is a git repository (or a local directory) that contains
//! one or more `extension.toml` trees. Repositories can be distributed like
//! Nix flakes: one repository owns many packages, and a checkout is selected
//! by URL + optional ref + optional subdirectory. An optional `wm-flake.toml`
//! at the source root lists packages explicitly:
//!
//! ```toml
//! [flake]
//! name = "acme-extensions"
//! version = "1"
//!
//! [[package]]
//! id = "chat_gui"
//! path = "packages/chat_gui"
//! ```
//!
//! Without `wm-flake.toml`, every `extension.toml` under the checkout is a
//! package. Installs copy the package directory into
//! `<data_dir>/extensions/<id>/`, so the existing control-plane loader sees it
//! after a manifest reload. The lockfile
//! `<data_dir>/extensions/.packages.json` records the source URL, ref, commit
//! and relative path for reproducible updates.
//!
//! This is deliberately a local, operator-invoked tool. It never auto-runs
//! code from a source: the caller must pass `--yes`, and the manifest/hooks are
//! the same trusted-admin surface the runtime already has.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use wm_ext::{ExtensionManifest, valid_id};

use crate::discovery::collect_manifests;

const LOCK_VERSION: u32 = 1;
const SKIPPED_COPY_DIRS: &[&str] = &[".git", "target", "node_modules", ".pnpm-store"];

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(clap::Args)]
pub struct PackagesArgs {
    #[command(subcommand)]
    pub action: PackagesAction,
}

#[derive(clap::Subcommand)]
pub enum PackagesAction {
    /// Lista os pacotes instalados e a origem de cada um.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Descobre os pacotes de um repositório git sem instalar.
    Discover {
        /// URL git, `github:owner/repo` ou diretório local.
        source: String,
        /// Branch, tag ou commit da origem.
        #[arg(long)]
        r#ref: Option<String>,
        /// Subdiretório do repositório que contém os pacotes.
        #[arg(long)]
        subdir: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Instala um ou mais pacotes de um repositório git.
    Install {
        source: String,
        /// Branch, tag ou commit da origem.
        #[arg(long)]
        r#ref: Option<String>,
        #[arg(long)]
        subdir: Option<String>,
        /// Instala somente este id (obrigatório quando o repositório tem vários).
        #[arg(long)]
        package: Option<String>,
        /// Instala todos os pacotes descobertos.
        #[arg(long, conflicts_with = "package")]
        all: bool,
        /// Nome local do checkout em .sources (default: derivado da URL).
        #[arg(long)]
        name: Option<String>,
        /// Confirma a execução de código do pacote no nó.
        #[arg(long)]
        yes: bool,
        /// Substitui um pacote já instalado.
        #[arg(long)]
        force: bool,
        /// Mostra o que seria feito, sem copiar/gravar.
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove um pacote instalado pelo gerenciador.
    Remove {
        id: String,
        /// Remove apenas o registro do lock, mantendo os arquivos.
        #[arg(long)]
        keep_files: bool,
        #[arg(long)]
        yes: bool,
    },
    /// Atualiza um ou todos os pacotes para o commit atual da origem.
    Update {
        id: Option<String>,
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// Lista as origens registradas no lock.
    Sources {
        #[arg(long)]
        json: bool,
    },
}

pub fn run(root: &Path, action: PackagesAction) -> anyhow::Result<()> {
    match action {
        PackagesAction::List { json } => list(root, json),
        PackagesAction::Discover {
            source,
            r#ref,
            subdir,
            json,
        } => discover(root, &source, r#ref.as_deref(), subdir.as_deref(), json),
        PackagesAction::Install {
            source,
            r#ref,
            subdir,
            package,
            all,
            name,
            yes,
            force,
            dry_run,
        } => install(
            root,
            &source,
            r#ref.as_deref(),
            subdir.as_deref(),
            package.as_deref(),
            all,
            name.as_deref(),
            yes,
            force,
            dry_run,
        ),
        PackagesAction::Remove {
            id,
            keep_files,
            yes,
        } => remove(root, &id, keep_files, yes),
        PackagesAction::Update { id, yes, dry_run } => {
            update(root, id.as_deref(), yes, dry_run)
        }
        PackagesAction::Sources { json } => sources(root, json),
    }
}

// ---------------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InstalledPackage {
    id: String,
    name: String,
    version: String,
    source: String,
    #[serde(default)]
    git_ref: Option<String>,
    commit: String,
    subdir: String,
    installed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SourceRecord {
    url: String,
    #[serde(default)]
    git_ref: Option<String>,
    #[serde(default)]
    commit: Option<String>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Lockfile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    packages: BTreeMap<String, InstalledPackage>,
    #[serde(default)]
    sources: BTreeMap<String, SourceRecord>,
}

impl Lockfile {
    fn normalized(mut self) -> Self {
        if self.version == 0 {
            self.version = LOCK_VERSION;
        }
        self
    }
}

#[derive(Debug, Clone)]
struct Candidate {
    id: String,
    name: String,
    version: String,
    description: String,
    visibility: String,
    driver: String,
    path: PathBuf,
    subdir: String,
}

#[derive(Debug, Serialize)]
struct CandidateView<'a> {
    id: &'a str,
    name: &'a str,
    version: &'a str,
    description: &'a str,
    visibility: &'a str,
    driver: &'a str,
    path: String,
    source: &'a str,
}

struct SourceCheckout {
    dir: PathBuf,
    url: String,
    commit: String,
    source_name: String,
}

#[derive(Debug, Deserialize, Default)]
struct FlakeFile {
    #[serde(default)]
    package: Vec<FlakePackage>,
}

#[derive(Debug, Deserialize)]
struct FlakePackage {
    id: String,
    path: String,
    #[serde(default)]
    description: String,
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

fn list(root: &Path, json: bool) -> anyhow::Result<()> {
    let lock = load_lock(root)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&lock.packages)?);
        return Ok(());
    }
    if lock.packages.is_empty() {
        println!("nenhum pacote instalado pelo gerenciador");
        return Ok(());
    }
    println!(
        "{:<24} {:<10} {:<12} {:<10} {}",
        "ID", "VERSÃO", "COMMIT", "ORIGEM", "SUBDIR"
    );
    for package in lock.packages.values() {
        println!(
            "{:<24} {:<10} {:<12} {:<10} {}",
            package.id,
            package.version,
            short_commit(&package.commit),
            package.source,
            package.subdir
        );
    }
    println!("\n{} pacote(s).", lock.packages.len());
    Ok(())
}

fn sources(root: &Path, json: bool) -> anyhow::Result<()> {
    let lock = load_lock(root)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&lock.sources)?);
        return Ok(());
    }
    if lock.sources.is_empty() {
        println!("nenhuma origem registrada");
        return Ok(());
    }
    println!("{:<24} {:<10} {:<12} {}", "NOME", "REF", "COMMIT", "URL");
    for (name, source) in &lock.sources {
        println!(
            "{:<24} {:<10} {:<12} {}",
            name,
            source.git_ref.as_deref().unwrap_or("HEAD"),
            source
                .commit
                .as_deref()
                .map(short_commit)
                .unwrap_or("—"),
            source.url
        );
    }
    Ok(())
}

fn discover(
    root: &Path,
    source: &str,
    git_ref: Option<&str>,
    subdir: Option<&str>,
    json: bool,
) -> anyhow::Result<()> {
    let checkout = checkout(root, source, git_ref, None, subdir)?;
    let package_root = package_root(&checkout.dir, subdir)?;
    let candidates = discover_candidates(&package_root, &checkout.url)?;
    if json {
        let views: Vec<CandidateView<'_>> = candidates.iter().map(candidate_view).collect();
        println!("{}", serde_json::to_string_pretty(&views)?);
        return Ok(());
    }
    print_candidates(&candidates, &checkout);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn install(
    root: &Path,
    source: &str,
    git_ref: Option<&str>,
    subdir: Option<&str>,
    package: Option<&str>,
    all: bool,
    name: Option<&str>,
    yes: bool,
    force: bool,
    dry_run: bool,
) -> anyhow::Result<()> {
    if !yes && !dry_run {
        bail!(
            "instalar um pacote executa código do repositório no nó; repita com --yes para confirmar"
        );
    }

    let checkout = checkout(root, source, git_ref, name, subdir)?;
    let package_root = package_root(&checkout.dir, subdir)?;
    let candidates = discover_candidates(&package_root, &checkout.url)?;
    let selected = select_candidates(&candidates, package, all)?;

    if dry_run {
        print_candidates_refs(&selected, &checkout);
        println!("\n(dry-run) nada foi copiado; use sem --dry-run e com --yes para instalar.");
        return Ok(());
    }

    let extensions_dir = extensions_dir(root);
    let mut lock = load_lock(root)?;
    for candidate in selected {
        if !valid_id(&candidate.id) {
            bail!("id de pacote inválido: {}", candidate.id);
        }
        let destination = extensions_dir.join(&candidate.id);
        ensure_child(&extensions_dir, &destination)?;
        if destination.exists() {
            if !force {
                bail!(
                    "já existe uma extensão '{}' em {}; use --force para substituir",
                    candidate.id,
                    destination.display()
                );
            }
            if destination.symlink_metadata()?.file_type().is_symlink() {
                bail!("destino é um symlink: {}", destination.display());
            }
            fs::remove_dir_all(&destination)
                .with_context(|| format!("removendo {}", destination.display()))?;
        }
        copy_dir_all(&candidate.path, &destination)
            .with_context(|| format!("copiando pacote {} para {}", candidate.id, destination.display()))?;

        lock.packages.insert(
            candidate.id.clone(),
            InstalledPackage {
                id: candidate.id.clone(),
                name: candidate.name.clone(),
                version: candidate.version.clone(),
                source: checkout.url.clone(),
                git_ref: git_ref.map(str::to_string),
                commit: checkout.commit.clone(),
                subdir: candidate.subdir.clone(),
                installed_at: Utc::now(),
            },
        );
        lock.sources.insert(
            checkout.source_name.clone(),
            SourceRecord {
                url: checkout.url.clone(),
                git_ref: git_ref.map(str::to_string),
                commit: Some(checkout.commit.clone()),
                updated_at: Utc::now(),
            },
        );
        println!(
            "instalado {} ({}) em {}",
            candidate.id,
            short_commit(&checkout.commit),
            destination.display()
        );
    }
    save_lock(root, &lock)?;
    println!(
        "\nRecarregue os manifestos no painel (Extensões → Recarregar manifestos) para o control plane ver os pacotes."
    );
    Ok(())
}

fn remove(root: &Path, id: &str, keep_files: bool, yes: bool) -> anyhow::Result<()> {
    if !yes {
        bail!("remover um pacote apaga arquivos do nó; repita com --yes para confirmar");
    }
    let mut lock = load_lock(root)?;
    if lock.packages.remove(id).is_none() {
        bail!("pacote '{id}' não está registrado no lock");
    }
    if !keep_files {
        let extensions_dir = extensions_dir(root);
        let destination = extensions_dir.join(id);
        ensure_child(&extensions_dir, &destination)?;
        if destination.exists() {
            if destination.symlink_metadata()?.file_type().is_symlink() {
                bail!("destino é um symlink: {}", destination.display());
            }
            fs::remove_dir_all(&destination)
                .with_context(|| format!("removendo {}", destination.display()))?;
        }
        println!("removido {}", destination.display());
    }
    save_lock(root, &lock)?;
    Ok(())
}

fn update(root: &Path, id: Option<&str>, yes: bool, dry_run: bool) -> anyhow::Result<()> {
    if !yes && !dry_run {
        bail!("atualizar pacotes executa código do repositório no nó; repita com --yes");
    }
    let lock = load_lock(root)?;
    let selected: Vec<InstalledPackage> = match id {
        Some(id) => vec![
            lock.packages
                .get(id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("pacote '{id}' não instalado"))?,
        ],
        None => lock.packages.values().cloned().collect(),
    };
    if selected.is_empty() {
        println!("nenhum pacote instalado");
        return Ok(());
    }

    let mut failed = 0;
    for package in selected {
        let result = reinstall_one(root, &package, dry_run);
        if let Err(err) = result {
            failed += 1;
            eprintln!("erro ao atualizar {}: {err}", package.id);
        }
    }
    if failed > 0 {
        bail!("{failed} pacote(s) falharam ao atualizar");
    }
    Ok(())
}

fn reinstall_one(root: &Path, package: &InstalledPackage, dry_run: bool) -> anyhow::Result<()> {
    let checkout = checkout(
        root,
        &package.source,
        package.git_ref.as_deref(),
        Some(&package.id),
        Some(&package.subdir),
    )?;
    let package_root = package_root(&checkout.dir, Some(&package.subdir))?;
    let candidates = discover_candidates(&package_root, &checkout.url)?;
    let candidate = candidates
        .into_iter()
        .find(|candidate| candidate.id == package.id)
        .ok_or_else(|| anyhow::anyhow!("pacote '{}' sumiu da origem", package.id))?;

    if checkout.commit == package.commit {
        println!("{}: já atualizado ({})", package.id, short_commit(&checkout.commit));
        return Ok(());
    }
    if dry_run {
        println!(
            "{}: {} -> {}",
            package.id,
            short_commit(&package.commit),
            short_commit(&checkout.commit)
        );
        return Ok(());
    }

    let extensions_dir = extensions_dir(root);
    let destination = extensions_dir.join(&candidate.id);
    ensure_child(&extensions_dir, &destination)?;
    if destination.exists() {
        fs::remove_dir_all(&destination)?;
    }
    copy_dir_all(&candidate.path, &destination)?;
    let mut lock = load_lock(root)?;
    if let Some(entry) = lock.packages.get_mut(&candidate.id) {
        entry.commit = checkout.commit.clone();
        entry.version = candidate.version.clone();
        entry.installed_at = Utc::now();
    }
    save_lock(root, &lock)?;
    println!(
        "{}: atualizado para {}",
        candidate.id,
        short_commit(&checkout.commit)
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Source checkout and discovery
// ---------------------------------------------------------------------------

fn checkout(
    root: &Path,
    source: &str,
    git_ref: Option<&str>,
    name: Option<&str>,
    _subdir: Option<&str>,
) -> anyhow::Result<SourceCheckout> {
    let normalized = normalize_source(source);
    let source_name = source_name(name, &normalized);

    if let Some(local) = local_source(&normalized) {
        let dir = local
            .canonicalize()
            .with_context(|| format!("origem local inválida: {}", local.display()))?;
        if !dir.is_dir() {
            bail!("origem local não é diretório: {}", dir.display());
        }
        let commit = git(&dir, &["rev-parse", "HEAD"]).unwrap_or_else(|_| "local".into());
        return Ok(SourceCheckout {
            dir,
            url: normalized,
            commit,
            source_name,
        });
    }

    let sources = extensions_dir(root).join(".sources");
    fs::create_dir_all(&sources)?;
    let dest = sources.join(&source_name);
    ensure_child(&sources, &dest)?;
    if dest.exists() {
        fs::remove_dir_all(&dest)
            .with_context(|| format!("limpando checkout {}", dest.display()))?;
    }

    let mut args: Vec<&str> = vec!["clone", "--depth", "1"];
    if let Some(git_ref) = git_ref {
        args.extend(["--branch", git_ref]);
    }
    args.extend([normalized.as_str(), dest.to_str().unwrap_or_default()]);
    let output = Command::new("git")
        .args(&args)
        .output()
        .context("git não encontrado; instale git no nó")?;
    if !output.status.success() {
        // A commit SHA is not a branch; retry by fetching the exact object.
        if let Some(git_ref) = git_ref {
            if is_commit(git_ref) {
                let _ = fs::remove_dir_all(&dest);
                git_clone_plain(&normalized, &dest)?;
                run_git(&dest, &["fetch", "--depth", "1", "origin", git_ref])?;
                run_git(&dest, &["checkout", "--force", "FETCH_HEAD"])?;
                let commit = git(&dest, &["rev-parse", "HEAD"])?;
                return Ok(SourceCheckout {
                    dir: dest,
                    url: normalized,
                    commit,
                    source_name,
                });
            }
        }
        bail!(
            "git clone falhou: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let commit = git(&dest, &["rev-parse", "HEAD"])?;
    Ok(SourceCheckout {
        dir: dest,
        url: normalized,
        commit,
        source_name,
    })
}

fn git_clone_plain(url: &str, dest: &Path) -> anyhow::Result<()> {
    let output = Command::new("git")
        .args(["clone", "--depth", "1", url, dest.to_str().unwrap_or_default()])
        .output()
        .context("git não encontrado; instale git no nó")?;
    if !output.status.success() {
        bail!(
            "git clone falhou: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn package_root(dir: &Path, subdir: Option<&str>) -> anyhow::Result<PathBuf> {
    let root = match subdir {
        Some(subdir) => safe_join(dir, subdir)?,
        None => dir.to_path_buf(),
    };
    if !root.is_dir() {
        bail!("subdiretório de pacotes não existe: {}", root.display());
    }
    Ok(root)
}

fn discover_candidates(root: &Path, source: &str) -> anyhow::Result<Vec<Candidate>> {
    let flake_path = root.join("wm-flake.toml");
    let mut candidates = Vec::new();
    if flake_path.is_file() {
        let text = fs::read_to_string(&flake_path)?;
        let flake: FlakeFile = toml::from_str(&text)
            .with_context(|| format!("wm-flake.toml inválido em {}", flake_path.display()))?;
        if flake.package.is_empty() {
            bail!("{} não declara nenhum [[package]]", flake_path.display());
        }
        for package in flake.package {
            let package_dir = safe_join(root, &package.path)?;
            let manifest_path = package_dir.join("extension.toml");
            let manifest = read_manifest(&manifest_path, &package.id)?;
            let mut candidate = candidate_from(manifest, &package_dir, root)?;
            if candidate.description.trim().is_empty() && !package.description.trim().is_empty() {
                candidate.description = package.description.trim().to_string();
            }
            candidates.push(candidate);
        }
    } else {
        let mut manifests = Vec::new();
        let mut warnings = Vec::new();
        collect_manifests(root, 0, &mut manifests, &mut warnings);
        for path in manifests {
            let dir_id = path
                .parent()
                .and_then(|parent| parent.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let manifest = read_manifest(&path, &dir_id)?;
            let package_dir = path.parent().unwrap_or(root);
            candidates.push(candidate_from(manifest, package_dir, root)?);
        }
        for warning in warnings {
            eprintln!("aviso: {warning}");
        }
    }

    let mut seen = HashSet::new();
    candidates.retain(|candidate| {
        if seen.insert(candidate.id.clone()) {
            true
        } else {
            eprintln!("aviso: pacote duplicado '{}' ignorado", candidate.id);
            false
        }
    });
    if candidates.is_empty() {
        bail!("nenhum pacote encontrado em {source}");
    }
    candidates.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(candidates)
}

fn read_manifest(path: &Path, expected_id: &str) -> anyhow::Result<ExtensionManifest> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("lendo manifesto {}", path.display()))?;
    let manifest: ExtensionManifest = toml::from_str(&text)
        .with_context(|| format!("TOML inválido em {}", path.display()))?;
    manifest
        .validate()
        .map_err(|err| anyhow::anyhow!("manifesto inválido em {}: {err}", path.display()))?;
    if manifest.id != expected_id {
        bail!(
            "id '{}' não confere com o diretório '{}' em {}",
            manifest.id,
            expected_id,
            path.display()
        );
    }
    Ok(manifest)
}

fn candidate_from(
    manifest: ExtensionManifest,
    package_dir: &Path,
    root: &Path,
) -> anyhow::Result<Candidate> {
    let subdir = package_dir
        .strip_prefix(root)
        .unwrap_or(package_dir)
        .to_string_lossy()
        .into_owned();
    Ok(Candidate {
        id: manifest.id,
        name: manifest.name,
        version: manifest.version,
        description: manifest.description,
        visibility: manifest.visibility.as_str().to_string(),
        driver: manifest.node_driver.as_str().to_string(),
        path: package_dir.to_path_buf(),
        subdir: if subdir.is_empty() { ".".into() } else { subdir },
    })
}

fn select_candidates<'a>(
    candidates: &'a [Candidate],
    package: Option<&str>,
    all: bool,
) -> anyhow::Result<Vec<&'a Candidate>> {
    if let Some(id) = package {
        let candidate = candidates
            .iter()
            .find(|candidate| candidate.id == id)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "pacote '{id}' não encontrado; disponíveis: {}",
                    candidate_ids(candidates)
                )
            })?;
        return Ok(vec![candidate]);
    }
    if all {
        return Ok(candidates.iter().collect());
    }
    if candidates.len() == 1 {
        return Ok(vec![&candidates[0]]);
    }
    bail!(
        "o repositório tem {} pacotes; use --package <id> ou --all. Disponíveis: {}",
        candidates.len(),
        candidate_ids(candidates)
    )
}

fn print_candidates(candidates: &[Candidate], checkout: &SourceCheckout) {
    let refs: Vec<&Candidate> = candidates.iter().collect();
    print_candidates_refs(&refs, checkout);
}

fn print_candidates_refs(candidates: &[&Candidate], checkout: &SourceCheckout) {
    println!(
        "origem: {} @ {}",
        checkout.url,
        short_commit(&checkout.commit)
    );
    println!(
        "{:<24} {:<9} {:<10} {:<10} {}",
        "ID", "VISIB", "DRIVER", "VERSÃO", "SUBDIR"
    );
    for candidate in candidates {
        println!(
            "{:<24} {:<9} {:<10} {:<10} {}",
            candidate.id,
            candidate.visibility,
            candidate.driver,
            candidate.version,
            candidate.subdir
        );
    }
}

fn candidate_view(candidate: &Candidate) -> CandidateView<'_> {
    CandidateView {
        id: &candidate.id,
        name: &candidate.name,
        version: &candidate.version,
        description: &candidate.description,
        visibility: &candidate.visibility,
        driver: &candidate.driver,
        path: candidate.path.display().to_string(),
        source: &candidate.subdir,
    }
}

fn candidate_ids(candidates: &[Candidate]) -> String {
    candidates
        .iter()
        .map(|candidate| candidate.id.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------------
// Filesystem, git and lock helpers
// ---------------------------------------------------------------------------

fn load_lock(root: &Path) -> anyhow::Result<Lockfile> {
    let path = lock_path(root);
    match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str::<Lockfile>(&text)
            .map(Lockfile::normalized)
            .with_context(|| format!("lockfile inválido em {}", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Lockfile {
            version: LOCK_VERSION,
            ..Lockfile::default()
        }),
        Err(err) => Err(err).with_context(|| format!("lendo {}", path.display())),
    }
}

fn save_lock(root: &Path, lockfile: &Lockfile) -> anyhow::Result<()> {
    let path = lock_path(root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_vec_pretty(lockfile)?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, text)?;
    fs::rename(&temporary, &path)?;
    Ok(())
}

fn lock_path(root: &Path) -> PathBuf {
    extensions_dir(root).join(".packages.json")
}

fn extensions_dir(root: &Path) -> PathBuf {
    root.join("extensions")
}

fn copy_dir_all(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if SKIPPED_COPY_DIRS.contains(&name.as_str()) {
            continue;
        }
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        let source = entry.path();
        let destination = to.join(&name);
        if file_type.is_dir() {
            copy_dir_all(&source, &destination)?;
        } else if file_type.is_file() {
            fs::copy(&source, &destination)?;
        }
    }
    Ok(())
}

fn ensure_child(parent: &Path, child: &Path) -> anyhow::Result<()> {
    let parent = parent
        .canonicalize()
        .unwrap_or_else(|_| parent.to_path_buf());
    let child_abs = if child.is_absolute() {
        child.to_path_buf()
    } else {
        parent.join(child)
    };
    let normalized = normalize_path(&child_abs);
    if !normalized.starts_with(&parent) {
        bail!(
            "caminho fora de {}: {}",
            parent.display(),
            child.display()
        );
    }
    Ok(())
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn safe_join(base: &Path, relative: &str) -> anyhow::Result<PathBuf> {
    let relative = Path::new(relative);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::RootDir | Component::Prefix(_)))
    {
        bail!("caminho relativo inválido: {relative:?}");
    }
    let path = base.join(relative);
    let normalized = normalize_path(&path);
    if !normalized.starts_with(base) {
        bail!("caminho escapa de {}: {relative:?}", base.display());
    }
    Ok(normalized)
}

fn git(dir: &Path, args: &[&str]) -> anyhow::Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .context("git não encontrado; instale git no nó")?;
    if !output.status.success() {
        bail!(
            "git {} falhou: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn run_git(dir: &Path, args: &[&str]) -> anyhow::Result<()> {
    git(dir, args).map(|_| ())
}

fn normalize_source(source: &str) -> String {
    if let Some(rest) = source.strip_prefix("github:") {
        return format!("https://github.com/{rest}.git");
    }
    if let Some(rest) = source.strip_prefix("gitlab:") {
        return format!("https://gitlab.com/{rest}.git");
    }
    source.to_string()
}

fn local_source(source: &str) -> Option<PathBuf> {
    if let Some(path) = source.strip_prefix("file://") {
        return Some(PathBuf::from(path));
    }
    let path = Path::new(source);
    if path.is_dir() {
        return Some(path.to_path_buf());
    }
    None
}

fn source_name(name: Option<&str>, source: &str) -> String {
    let raw = name.unwrap_or(source);
    let stripped = raw
        .trim_end_matches(".git")
        .rsplit(['/', ':'])
        .next()
        .unwrap_or(raw);
    let mut slug = String::new();
    for ch in stripped.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "source".into()
    } else {
        slug.truncate(48);
        slug
    }
}

fn short_commit(commit: &str) -> &str {
    if commit.len() > 12 { &commit[..12] } else { commit }
}

fn is_commit(raw: &str) -> bool {
    raw.len() >= 7 && raw.len() <= 40 && raw.chars().all(|ch| ch.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_names_are_stable_and_multiplatform() {
        assert_eq!(
            source_name(None, "https://github.com/acme/extensions.git"),
            "extensions"
        );
        assert_eq!(
            source_name(None, "git@github.com:acme/my-ext.git"),
            "my-ext"
        );
        assert_eq!(source_name(Some("custom"), "https://example/repo"), "custom");
        assert_eq!(normalize_source("github:acme/ext"), "https://github.com/acme/ext.git");
    }

    #[test]
    fn safe_join_rejects_escapes() {
        let root = Path::new("/tmp/wm-packages");
        assert!(safe_join(root, "../etc").is_err());
        assert!(safe_join(root, "/etc").is_err());
        assert_eq!(safe_join(root, "packages/chat").unwrap(), root.join("packages/chat"));
    }

    #[test]
    fn flake_file_lists_and_validates_packages() {
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("packages/chat_gui");
        fs::create_dir_all(&package).unwrap();
        fs::write(
            package.join("extension.toml"),
            "id = \"chat_gui\"\nname = \"Chat\"\nnode_driver = \"exec\"\n[default_config]\ncommand = \"/bin/true\"\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("wm-flake.toml"),
            "[flake]\nname = \"test\"\n\n[[package]]\nid = \"chat_gui\"\npath = \"packages/chat_gui\"\n",
        )
        .unwrap();

        let candidates = discover_candidates(dir.path(), "test").unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].id, "chat_gui");
        assert_eq!(candidates[0].subdir, "packages/chat_gui");
        assert_eq!(candidates[0].driver, "exec");
    }
}
