use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use chrono::Utc;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use wm_proto::RelayClaims;

pub const SESSION_COOKIE: &str = "wm_session";
pub const SESSION_TTL_SECS: i64 = 12 * 60 * 60;

pub fn hash_password(password: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| anyhow::anyhow!("hash de senha: {e}"))
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

fn random_bytes<const N: usize>() -> anyhow::Result<[u8; N]> {
    let mut buf = [0u8; N];
    getrandom::fill(&mut buf).context("getrandom")?;
    Ok(buf)
}

pub fn random_token(prefix: &str) -> anyhow::Result<String> {
    let bytes: [u8; 32] = random_bytes()?;
    Ok(format!("{prefix}{}", URL_SAFE_NO_PAD.encode(bytes)))
}

pub fn random_password(len_bytes: usize) -> String {
    let mut buf = vec![0u8; len_bytes.max(8)];
    if getrandom::fill(&mut buf).is_err() {
        // Extremely unlikely; derive from UUID entropy as a last resort.
        buf = Uuid::new_v4().as_bytes().to_vec();
    }
    URL_SAFE_NO_PAD.encode(buf)
}

pub fn hash_token(token: &str) -> String {
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    hex_encode(&h.finalize())
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

pub fn new_control_signing_key() -> SigningKey {
    SigningKey::generate(&mut OsRng)
}

pub fn signing_key_from_b64(seed: &str) -> anyhow::Result<SigningKey> {
    let bytes = URL_SAFE_NO_PAD
        .decode(seed)
        .or_else(|_| STANDARD.decode(seed))
        .context("chave de assinatura inválida")?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("chave de assinatura precisa ter 32 bytes"))?;
    Ok(SigningKey::from_bytes(&arr))
}

pub fn signing_key_to_b64(key: &SigningKey) -> String {
    URL_SAFE_NO_PAD.encode(key.to_bytes())
}

pub fn public_key_b64(key: &SigningKey) -> String {
    URL_SAFE_NO_PAD.encode(key.verifying_key().to_bytes())
}

pub fn sign_relay_token(
    key: &SigningKey,
    node_id: Uuid,
    audience: &str,
    ttl_secs: i64,
) -> anyhow::Result<String> {
    let now = Utc::now().timestamp();
    let claims = RelayClaims {
        sub: node_id,
        aud: audience.to_string(),
        iat: now,
        exp: now + ttl_secs.clamp(60, 24 * 60 * 60),
    };
    let payload = serde_json::to_vec(&claims)?;
    let signature = key.sign(&payload);
    Ok(format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(payload),
        URL_SAFE_NO_PAD.encode(signature.to_bytes())
    ))
}

pub fn verify_relay_token(
    public_key_b64: &str,
    token: &str,
    expected_audience: &str,
) -> anyhow::Result<RelayClaims> {
    let (payload_b64, sig_b64) = token
        .split_once('.')
        .ok_or_else(|| anyhow::anyhow!("token de relay malformado"))?;
    let payload = URL_SAFE_NO_PAD
        .decode(payload_b64)
        .context("payload do relay inválido")?;
    let sig_bytes = URL_SAFE_NO_PAD
        .decode(sig_b64)
        .context("assinatura do relay inválida")?;
    let sig_arr: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("assinatura do relay inválida"))?;
    let pub_bytes = URL_SAFE_NO_PAD
        .decode(public_key_b64)
        .context("chave pública do control inválida")?;
    let pub_arr: [u8; 32] = pub_bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("chave pública do control inválida"))?;
    let verifying = VerifyingKey::from_bytes(&pub_arr)?;
    let signature = Signature::from_bytes(&sig_arr);
    verifying
        .verify(&payload, &signature)
        .map_err(|_| anyhow::anyhow!("token de relay com assinatura inválida"))?;
    let claims: RelayClaims = serde_json::from_slice(&payload)?;
    if claims.exp < Utc::now().timestamp() {
        bail!("token de relay expirado");
    }
    if claims.aud != expected_audience {
        bail!("token de relay para outra audiência");
    }
    Ok(claims)
}

pub fn parse_cookie(headers: &http::HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(http::header::COOKIE)?.to_str().ok()?;
    for part in raw.split(';') {
        let part = part.trim();
        if let Some((k, v)) = part.split_once('=') {
            if k == name {
                return Some(v.to_string());
            }
        }
    }
    None
}

pub fn session_cookie(value: &str, max_age: i64, secure: bool) -> String {
    format!(
        "{name}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}",
        name = SESSION_COOKIE,
        secure = if secure { "; Secure" } else { "" }
    )
}

pub fn clear_session_cookie(secure: bool) -> String {
    session_cookie("", 0, secure)
}

pub fn bearer_token(headers: &http::HeaderMap) -> Option<&str> {
    headers
        .get(http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

/// Small in-memory fixed-window limiter. Good enough for a single control node;
/// a reverse proxy should add its own limits in front of the public URL.
#[derive(Debug)]
pub struct RateLimiter {
    map: std::sync::Mutex<HashMap<String, (Instant, u32)>>,
    max: u32,
    window: Duration,
}

impl RateLimiter {
    pub fn new(max: u32, window: Duration) -> Self {
        Self {
            map: std::sync::Mutex::new(HashMap::new()),
            max,
            window,
        }
    }

    pub fn allow(&self, key: &str) -> bool {
        let now = Instant::now();
        let Ok(mut map) = self.map.lock() else {
            return false;
        };
        if self.max == 0 || key.len() > 128 {
            return false;
        }
        if !map.contains_key(key) && map.len() >= 20_000 {
            map.retain(|_, (start, _)| now.duration_since(*start) < self.window);
            // Do not evict live buckets: that would reset limits for an attacker.
            if map.len() >= 20_000 {
                return false;
            }
        }
        let entry = map.entry(key.to_string()).or_insert((now, 0));
        if now.duration_since(entry.0) >= self.window {
            *entry = (now, 1);
            true
        } else if entry.1 < self.max {
            entry.1 += 1;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords_roundtrip() {
        let hash = hash_password("segredo").unwrap();
        assert!(verify_password("segredo", &hash));
        assert!(!verify_password("errado", &hash));
    }

    #[test]
    fn relay_token_roundtrip() {
        let key = new_control_signing_key();
        let node = Uuid::new_v4();
        let pubkey = public_key_b64(&key);
        let aud = "https://mesh.example.com/relay";
        let token = sign_relay_token(&key, node, aud, 3600).unwrap();
        let claims = verify_relay_token(&pubkey, &token, aud).unwrap();
        assert_eq!(claims.sub, node);
        assert!(verify_relay_token(&pubkey, &token, "https://other/relay").is_err());
    }

    #[test]
    fn limiter_bounds_memory_and_never_resets_live_buckets() {
        let limiter = RateLimiter::new(1, Duration::from_secs(60));
        for index in 0..20_000 {
            assert!(limiter.allow(&format!("key-{index}")));
        }
        assert!(!limiter.allow("overflow"));
        assert!(!limiter.allow("key-0"));
        assert_eq!(limiter.map.lock().unwrap().len(), 20_000);
        let expired = Instant::now() - Duration::from_secs(61);
        limiter.map.lock().unwrap().get_mut("key-0").unwrap().0 = expired;
        assert!(limiter.allow("replacement"));
        assert_eq!(limiter.map.lock().unwrap().len(), 20_000);
        assert!(!limiter.allow(&"x".repeat(129)));
        assert!(!RateLimiter::new(0, Duration::ZERO).allow("disabled"));
    }

    #[test]
    fn limiter_blocks_after_max() {
        let limiter = RateLimiter::new(2, Duration::from_secs(60));
        assert!(limiter.allow("a"));
        assert!(limiter.allow("a"));
        assert!(!limiter.allow("a"));
        assert!(limiter.allow("b"));
    }
}
