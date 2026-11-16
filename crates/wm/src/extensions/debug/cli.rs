//! Non-interactive local diagnostics for future command integration.
//!
//! No terminal, HTTP server, background sampler or discovery-cache writes.

use std::path::Path;
use super::core::{Sampler, Snapshot};

/// Collect one local snapshot rooted at `root` (the WM data directory).
/// CPU/network rates are first-sample estimates; use a retained `core::Sampler`
/// for interval-based rates. Discovery status is relative to this invocation.
/// This synchronous host inspection may be put in `spawn_blocking` by async callers.
pub fn snapshot(root: &Path) -> anyhow::Result<Snapshot> {
    Ok(Sampler::read_only(root)?.sample())
}

/// Collect and serialize a snapshot as JSON, without printing or writing files.
/// `pretty` selects indented JSON. The caller controls stdout and exit status.
pub fn snapshot_json(root: &Path, pretty: bool) -> anyhow::Result<String> {
    let snapshot = snapshot(root)?;
    Ok(if pretty {
        serde_json::to_string_pretty(&snapshot)?
    } else {
        serde_json::to_string(&snapshot)?
    })
}
