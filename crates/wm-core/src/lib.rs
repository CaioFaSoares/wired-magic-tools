//! wired-magic agent: WireGuard data plane, control client, discovery, relay
//! client and local API.

pub mod agent;
pub mod control;
pub mod expose;
pub mod extensions;
pub mod local;
pub mod packet;
pub mod relay_client;
pub mod state;
pub mod stun;
pub mod tun;
pub mod wireguard;

pub use agent::{AgentSnapshot, run, run_until};
pub use state::{AgentConfig, NodeState, join};
