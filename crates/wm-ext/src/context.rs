//! Read-only host/mesh context returned by the local WM API.
//! Each enrolled server has its own agent socket. Call `extension_context` on
//! each explicitly selected profile to compose a multi-server view; credentials
//! and cross-server activation authority are deliberately not exported.
use serde::{Deserialize, Serialize};
use wm_proto::{AgentStatus, ExtensionStatus, PeerStatus, ShareInfo};

pub const CONTEXT_API_VERSION: u32 = 1;
pub const CONTEXT_METHOD: &str = "extension_context";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionContext {
    pub api_version: u32,
    pub host: AgentStatus,
    pub nodes: Vec<PeerStatus>,
    pub shares: Vec<ShareInfo>,
    pub extensions: Vec<ExtensionStatus>,
}

/// A multi-server application keeps provenance instead of merging node names
/// from unrelated networks. The caller supplies the stable profile identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerContext {
    pub profile: String,
    pub context: ExtensionContext,
}
