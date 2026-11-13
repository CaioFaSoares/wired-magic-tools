//! One-line onboarding endpoints.
//!
//! `GET /j/<join-token>` returns a shell script with the control URL and an
//! opaque `wmc_...` code baked in. `GET /download/wm` serves the agent binary
//! (or a cross-compiled file placed in `<data_dir>/dist`). Both are public by
//! design; the join token is the credential and is consumed on enrollment.

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use wm_proto::encode_connect_code;

use super::ApiErr;
use crate::state::AppState;

pub async fn script(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<Response, ApiErr> {
    let token = token.trim();
    if !valid_join_token(token) {
        return Err(ApiErr::bad_request(
            "invalid_token",
            "código de convite inválido",
        ));
    }
    let code = encode_connect_code(&state.cfg.public_url, token)
        .map_err(|err| ApiErr::bad_request("invalid_invite", err))?;
    let url = shell_quote(state.cfg.public_url.trim_end_matches('/'));
    let code = shell_quote(&code);
    let body = format!(
        r#"#!/bin/sh
set -eu

URL={url}
CODE={code}

case "$(uname -s)" in
  Linux)  os=linux ;;
  Darwin) os=macos ;;
  *) echo "sistema não suportado: $(uname -s)" >&2; exit 1 ;;
esac

case "$(uname -m)" in
  x86_64|amd64) arch=amd64 ;;
  arm64|aarch64) arch=arm64 ;;
  *) echo "arquitetura não suportada: $(uname -m)" >&2; exit 1 ;;
esac

tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT INT TERM

echo "wired-magic: baixando wm para $os/$arch..."
if ! curl -fsSL "$URL/download/wm?os=$os&arch=$arch" -o "$tmp"; then
  echo "não foi possível baixar o binário para $os/$arch." >&2
  echo "peça ao operador para publicar wm-$os-$arch em <data_dir>/dist/." >&2
  exit 1
fi
chmod 0755 "$tmp"

if [ "$(id -u)" -eq 0 ]; then
  install -m 0755 "$tmp" /usr/local/bin/wm
  /usr/local/bin/wm join --code "$CODE" --no-start
  /usr/local/bin/wm service install --mode agent
else
  sudo install -m 0755 "$tmp" /usr/local/bin/wm
  sudo /usr/local/bin/wm join --code "$CODE" --no-start
  sudo /usr/local/bin/wm service install --mode agent
fi

echo
echo "pronto. veja o estado com: sudo wm status"
echo "abra a GUI local com:       wm gui"
"#
    );
    let mut response = (StatusCode::OK, body).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        "text/x-shellscript; charset=utf-8"
            .parse()
            .expect("content type"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store, max-age=0".parse().unwrap());
    Ok(response)
}

#[derive(Debug, Deserialize)]
pub struct BinaryQuery {
    #[serde(default)]
    pub os: Option<String>,
    #[serde(default)]
    pub arch: Option<String>,
}

pub async fn binary(
    State(state): State<Arc<AppState>>,
    Query(query): Query<BinaryQuery>,
) -> Result<Response, ApiErr> {
    let current_os = normalize_os(std::env::consts::OS);
    let current_arch = normalize_arch(std::env::consts::ARCH);
    let raw_os = query
        .os
        .as_deref()
        .unwrap_or(current_os.as_str())
        .trim()
        .to_string();
    let raw_arch = query
        .arch
        .as_deref()
        .unwrap_or(current_arch.as_str())
        .trim()
        .to_string();
    if !safe_component(&raw_os) || !safe_component(&raw_arch) {
        return Err(ApiErr::bad_request("invalid_target", "os/arch inválidos"));
    }
    let os = normalize_os(&raw_os);
    let arch = normalize_arch(&raw_arch);

    let dist = state.cfg.data_dir.join("dist");
    let names = [
        dist.join(format!("wm-{os}-{arch}")),
        dist.join(format!("wm-{raw_os}-{raw_arch}")),
        dist.join(format!("wm-{os}-{arch}.bin")),
        dist.join("wm"),
    ];
    let mut file: Option<PathBuf> = names.iter().find(|path| path.is_file()).cloned();
    if file.is_none() && os == current_os && arch == current_arch {
        file = std::env::current_exe().ok().filter(|path| path.is_file());
    }
    let Some(file) = file else {
        return Err(ApiErr::not_found(
            "binary_unavailable",
            format!(
                "binário indisponível para {os}/{arch}; coloque wm-{os}-{arch} em {}",
                dist.display()
            ),
        ));
    };
    let bytes = tokio::fs::read(&file)
        .await
        .map_err(|err| ApiErr::internal(format!("lendo {}: {err}", file.display())))?;
    let mut response = (StatusCode::OK, Body::from(bytes)).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        "application/octet-stream".parse().unwrap(),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        "attachment; filename=\"wm\"".parse().unwrap(),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store, max-age=0".parse().unwrap());
    Ok(response)
}

fn valid_join_token(token: &str) -> bool {
    let bytes = token.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 128
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

fn normalize_os(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "darwin" | "macos" | "osx" => "macos".into(),
        "linux" => "linux".into(),
        other => other.to_string(),
    }
}

fn normalize_arch(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "arm64" | "aarch64" => "aarch64".into(),
        "x86_64" | "amd64" | "x64" => "x86_64".into(),
        other => other.to_string(),
    }
}

fn safe_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_escapes_single_quotes() {
        assert_eq!(shell_quote("a'b"), "'a'\"'\"'b'");
    }

    #[test]
    fn validates_targets() {
        assert!(valid_join_token("wmj_abc-123"));
        assert!(!valid_join_token("../etc/passwd"));
        assert!(safe_component("linux"));
        assert!(!safe_component("x/y"));
    }
}
