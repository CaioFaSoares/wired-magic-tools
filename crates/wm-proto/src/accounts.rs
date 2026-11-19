//! Human accounts are distinct from enrolled nodes and bootstrap credentials.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountRole {
    Admin,
    User,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountUser {
    pub id: Uuid,
    pub username: String,
    pub role: AccountRole,
}

// Deliberately no Debug: request bodies contain plaintext secrets.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountCredentials {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SetupStatus {
    pub required: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SetupLoginResponse {
    pub csrf_token: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupCompleteRequest {
    pub admin: AccountCredentials,
    pub user: AccountCredentials,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetupCompleteResponse {
    pub admin: AccountUser,
    pub user: AccountUser,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct AccountLoginResponse {
    pub user: AccountUser,
    pub csrf_token: String,
}
