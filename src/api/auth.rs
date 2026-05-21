use serde::Deserialize;

use crate::api::cache;

const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";

#[derive(Debug)]
pub struct AccessToken {
    pub token: String,
    pub expires_in: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("missing env var {0} — see ads/README.md (rezolutnie) or run `python -m ads.oauth_flow`")]
    MissingEnv(&'static str),
    #[error("HTTP error talking to oauth2.googleapis.com: {0}")]
    Http(#[from] reqwest::Error),
    #[error("oauth2.googleapis.com returned {status}: {body}")]
    TokenEndpoint { status: u16, body: String },
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

pub fn exchange_refresh_token() -> Result<AccessToken, AuthError> {
    let client_id = require_env("GOOGLE_ADS_CLIENT_ID")?;
    let client_secret = require_env("GOOGLE_ADS_CLIENT_SECRET")?;
    let refresh_token = require_env("GOOGLE_ADS_REFRESH_TOKEN")?;
    exchange_with(&client_id, &client_secret, &refresh_token)
}

fn exchange_with(
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
) -> Result<AccessToken, AuthError> {
    let form = [
        ("grant_type", "refresh_token"),
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("refresh_token", refresh_token),
    ];

    let response = reqwest::blocking::Client::new()
        .post(TOKEN_ENDPOINT)
        .form(&form)
        .send()?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().unwrap_or_default();
        return Err(AuthError::TokenEndpoint {
            status: status.as_u16(),
            body,
        });
    }

    let parsed: TokenResponse = response.json()?;
    Ok(AccessToken {
        token: parsed.access_token,
        expires_in: parsed.expires_in,
    })
}

/// Cache-aware variant: returns a cached access token if one is still valid
/// for the current refresh token, otherwise performs a fresh OAuth exchange
/// and writes the result to the project cache.
pub fn get_access_token() -> Result<AccessToken, AuthError> {
    let client_id = require_env("GOOGLE_ADS_CLIENT_ID")?;
    let client_secret = require_env("GOOGLE_ADS_CLIENT_SECRET")?;
    let refresh_token = require_env("GOOGLE_ADS_REFRESH_TOKEN")?;

    let cache_enabled = !cache::disabled_by_env();
    let cache_dir = cache::project_cache_dir();
    let fp = cache::fingerprint(&refresh_token);

    if cache_enabled {
        if let Some(cached) = cache::load_token(&cache_dir, &fp) {
            let expires_in = cached.expires_at.saturating_sub(cache::now_unix());
            return Ok(AccessToken {
                token: cached.access_token,
                expires_in,
            });
        }
    }

    let token = exchange_with(&client_id, &client_secret, &refresh_token)?;

    if cache_enabled {
        let _ = cache::save_token(
            &cache_dir,
            &cache::TokenCache {
                fingerprint: fp,
                access_token: token.token.clone(),
                expires_at: cache::now_unix().saturating_add(token.expires_in),
            },
        );
    }

    Ok(token)
}

fn require_env(name: &'static str) -> Result<String, AuthError> {
    match std::env::var(name) {
        Ok(v) if !v.is_empty() => Ok(v),
        _ => Err(AuthError::MissingEnv(name)),
    }
}
