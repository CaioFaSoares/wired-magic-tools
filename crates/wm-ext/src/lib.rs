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
//! format, the node driver configuration contracts and their validation. It
//! also exposes an asynchronous, typed remote admin client for CLI/TUI tooling.

pub mod admin;
pub use admin::{AdminClient, AdminClientOptions, AdminError, AdminSession};
pub mod cloudflared;
pub mod components;
pub mod context;
pub use context::{ExtensionContext, ServerContext, CONTEXT_API_VERSION, CONTEXT_METHOD};
pub mod preferences;
pub use preferences::{ExtensionPreferences, ExtensionPreferencesStore};
pub use components::{ExtensionComponents, ExtensionRegion};
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
    builtin_debug_gui_manifest, builtin_debug_tui_manifest, builtin_manifests,
    builtin_seance_manifest, load_dir, seed_builtin_manifests, valid_id,
};
