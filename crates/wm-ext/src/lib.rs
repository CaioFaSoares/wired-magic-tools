//! Extension SDK for wired-magic.
//!
//! An extension has two halves:
//!
//! * a **control side**, owned by `wm-control`: manifest, admin API, database
//!   tables and the desired state injected into each node's map;
//! * a **node side**, owned by `wm-core`: a supervised sidecar process that
//!   reconciles the desired state received in the map.
//!
//! This crate contains the pieces both halves share: the on-disk manifest
//! format, the node driver configuration contracts and their validation. It has
//! no async runtime and no I/O beyond the manifest loader, so it can be used by
//! tests and tooling as well as by both daemons.

pub mod cloudflared;
pub mod exec;
pub mod manifest;

pub use cloudflared::{
    CloudflaredConfig, CloudflaredMode, CloudflaredRouteConfig, CloudflaredTunnelConfig,
    extract_quick_tunnel_url, render_named_config, validate_binary, validate_credentials,
    validate_hostname, validate_local_url, validate_project, validate_tunnel_id,
};
pub use exec::{ExecConfig, RestartPolicy, validate_exec_config};
pub use manifest::{
    AdapterExpose, AdapterShare, AdapterSpec, CHAT_ID, CLOUDFLARED_DRIVER, CLOUDFLARED_ID,
    EXEC_DRIVER, ExtensionManifest, ExtensionVisibility, LoadedManifests, NodeDriver,
    builtin_chat_gui_manifest, builtin_chat_tui_manifest, builtin_cloudflared_manifest,
    builtin_debug_gui_manifest, builtin_debug_tui_manifest, builtin_seance_manifest, load_dir,
    seed_builtin_manifests, valid_id,
};
