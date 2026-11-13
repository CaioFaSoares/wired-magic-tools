use std::time::Duration;

use anyhow::{Context, bail};
use reqwest::StatusCode;
use uuid::Uuid;
use serde::de::DeserializeOwned;

use wm_proto::{
    ApiErrorBody, CreateShareRequest, EndpointsRequest, EnrollRequest, EnrollResponse,
    NetworkMap, NodeView, ReportRequest, ShareInfo, UpdateShareRequest, VersionInfo,
};

#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    #[error("falha de rede: {0}")]
    Http(#[from] reqwest::Error),
    #[error("API {status} ({code}): {message}")]
    Api {
        status: u16,
        code: String,
        message: String,
    },
    #[error("nó pendente de aprovação")]
    Pending,
    #[error("não autorizado")]
    Unauthorized,
    #[error("{0}")]
    Other(String),
}

impl ControlError {
    pub fn is_pending(&self) -> bool {
        matches!(self, ControlError::Pending)
    }
    pub fn is_unauthorized(&self) -> bool {
        matches!(self, ControlError::Unauthorized)
    }
}

#[derive(Clone)]
pub struct ControlClient {
    http: reqwest::Client,
    base: String,
}

impl ControlClient {
    pub fn new(control_url: &str) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(format!("wm/{}", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(45))
            .connect_timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self {
            http,
            base: control_url.trim_end_matches('/').to_string(),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    async fn parse<T: DeserializeOwned>(resp: reqwest::Response) -> Result<T, ControlError> {
        let status = resp.status();
        let bytes = resp.bytes().await?;
        if status.is_success() {
            return serde_json::from_slice(&bytes)
                .map_err(|e| ControlError::Other(format!("resposta inválida: {e}")));
        }
        let parsed: Option<ApiErrorBody> = serde_json::from_slice(&bytes).ok();
        let (code, message) = parsed
            .map(|b| (b.error.code, b.error.message))
            .unwrap_or_else(|| ("http_error".into(), String::from_utf8_lossy(&bytes).into()));
        if status == StatusCode::FORBIDDEN && code == "pending_approval" {
            return Err(ControlError::Pending);
        }
        if status == StatusCode::UNAUTHORIZED {
            return Err(ControlError::Unauthorized);
        }
        Err(ControlError::Api {
            status: status.as_u16(),
            code,
            message,
        })
    }

    pub async fn meta(&self) -> Result<VersionInfo, ControlError> {
        let resp = self.http.get(self.url("/api/v1/meta")).send().await?;
        Self::parse(resp).await
    }

    pub async fn enroll(&self, req: &EnrollRequest) -> Result<EnrollResponse, ControlError> {
        let resp = self
            .http
            .post(self.url("/api/v1/enroll"))
            .json(req)
            .send()
            .await?;
        Self::parse(resp).await
    }

    pub async fn map(
        &self,
        token: &str,
        version: u64,
        wait: u64,
    ) -> Result<NetworkMap, ControlError> {
        let resp = self
            .http
            .get(self.url("/api/v1/map"))
            .bearer_auth(token)
            .query(&[("version", version.to_string()), ("wait", wait.to_string())])
            .send()
            .await?;
        Self::parse(resp).await
    }

    pub async fn post_endpoints(
        &self,
        token: &str,
        req: &EndpointsRequest,
    ) -> Result<(), ControlError> {
        let resp = self
            .http
            .post(self.url("/api/v1/node/endpoints"))
            .bearer_auth(token)
            .json(req)
            .send()
            .await?;
        let _: serde_json::Value = Self::parse(resp).await?;
        Ok(())
    }

    pub async fn post_report(
        &self,
        token: &str,
        req: &ReportRequest,
    ) -> Result<(), ControlError> {
        let resp = self
            .http
            .post(self.url("/api/v1/node/report"))
            .bearer_auth(token)
            .json(req)
            .send()
            .await?;
        let _: serde_json::Value = Self::parse(resp).await?;
        Ok(())
    }

    pub async fn leave(&self, token: &str) -> Result<(), ControlError> {
        let resp = self
            .http
            .post(self.url("/api/v1/node/leave"))
            .bearer_auth(token)
            .send()
            .await?;
        let _: serde_json::Value = Self::parse(resp).await?;
        Ok(())
    }

    pub async fn list_shares(&self, token: &str) -> Result<Vec<ShareInfo>, ControlError> {
        let resp = self
            .http
            .get(self.url("/api/v1/node/shares"))
            .bearer_auth(token)
            .send()
            .await?;
        Self::parse(resp).await
    }

    pub async fn create_share(
        &self,
        token: &str,
        req: &CreateShareRequest,
    ) -> Result<ShareInfo, ControlError> {
        let resp = self
            .http
            .post(self.url("/api/v1/node/shares"))
            .bearer_auth(token)
            .json(req)
            .send()
            .await?;
        Self::parse(resp).await
    }

    pub async fn update_share(
        &self,
        token: &str,
        id: Uuid,
        req: &UpdateShareRequest,
    ) -> Result<ShareInfo, ControlError> {
        let resp = self
            .http
            .patch(self.url(&format!("/api/v1/node/shares/{id}")))
            .bearer_auth(token)
            .json(req)
            .send()
            .await?;
        Self::parse(resp).await
    }

    pub async fn delete_share(&self, token: &str, id: Uuid) -> Result<(), ControlError> {
        let resp = self
            .http
            .delete(self.url(&format!("/api/v1/node/shares/{id}")))
            .bearer_auth(token)
            .send()
            .await?;
        let _: serde_json::Value = Self::parse(resp).await?;
        Ok(())
    }

    pub async fn rename_self(&self, token: &str, name: &str) -> Result<NodeView, ControlError> {
        let resp = self
            .http
            .patch(self.url("/api/v1/node/self"))
            .bearer_auth(token)
            .json(&serde_json::json!({ "name": name }))
            .send()
            .await?;
        Self::parse(resp).await
    }

    pub fn websocket_url(base: &str, path: &str) -> anyhow::Result<String> {
        let trimmed = base.trim_end_matches('/');
        let ws = if let Some(rest) = trimmed.strip_prefix("https://") {
            format!("wss://{rest}")
        } else if let Some(rest) = trimmed.strip_prefix("http://") {
            format!("ws://{rest}")
        } else {
            bail!("URL de relay sem http/https: {base}");
        };
        Ok(format!("{ws}{path}"))
    }

    pub fn base(&self) -> &str {
        &self.base
    }
}

pub fn parse_control_url(raw: &str) -> anyhow::Result<String> {
    let trimmed = raw.trim().trim_end_matches('/');
    if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
        bail!("URL do control deve começar com http:// ou https://");
    }
    url::Url::parse(trimmed).context("URL do control inválida")?;
    Ok(trimmed.to_string())
}
