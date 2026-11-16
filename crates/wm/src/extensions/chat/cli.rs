//! Non-interactive async chat operations for future command integration.
//!
//! Callers own the reqwest client (and its timeout/auth/proxy policy). These
//! functions return typed, serializable values; they never print or start a UI.

use anyhow::{Context, bail};
use reqwest::{Client, Url};

use super::core::{InfoResponse, MessagesResponse, PostInput, PostResponse};

fn endpoint(base: &str, path: &str) -> anyhow::Result<Url> {
    let mut url = Url::parse(base).context("URL de chat inválida")?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        bail!("a URL precisa usar http:// ou https:// e indicar um host");
    }
    let prefix = url.path().trim_end_matches('/');
    url.set_path(&format!("{prefix}/{path}"));
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

/// Fetch room metadata with GET `/api/info`.
/// `base` may include a reverse-proxy path prefix. HTTP failures are errors.
pub async fn info(client: &Client, base: &str) -> anyhow::Result<InfoResponse> {
    client.get(endpoint(base, "api/info")?).send().await?
        .error_for_status()?.json().await.context("resposta inválida de /api/info")
}

/// List messages strictly after `after` (use zero for retained history).
/// Compare epochs between calls; reset the cursor to zero after a restart.
pub async fn list(client: &Client, base: &str, after: u64) -> anyhow::Result<MessagesResponse> {
    client.get(endpoint(base, "api/messages")?).query(&[("after", after)])
        .send().await?.error_for_status()?.json().await
        .context("resposta inválida de /api/messages")
}

/// Send one message with POST `/api/messages`; returns its server ID and epoch.
/// A missing/blank author becomes anonymous. The server validates message text.
/// This is the only operation here that mutates the remote room.
pub async fn send(client: &Client, base: &str, author: Option<&str>, text: &str) -> anyhow::Result<PostResponse> {
    let input = PostInput { author: author.map(str::to_owned), text: Some(text.to_owned()) };
    client.post(endpoint(base, "api/messages")?).json(&input)
        .send().await?.error_for_status()?.json().await
        .context("resposta inválida ao enviar mensagem")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_preserve_prefix_and_replace_query() {
        assert_eq!(endpoint("https://example.com/chat/?old=1#fragment", "api/info").unwrap().as_str(),
            "https://example.com/chat/api/info");
        assert!(endpoint("file:///tmp/chat", "api/info").is_err());
    }
}
