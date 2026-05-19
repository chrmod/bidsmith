use serde::Deserialize;

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

    let form = [
        ("grant_type", "refresh_token"),
        ("client_id", client_id.as_str()),
        ("client_secret", client_secret.as_str()),
        ("refresh_token", refresh_token.as_str()),
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

fn require_env(name: &'static str) -> Result<String, AuthError> {
    match std::env::var(name) {
        Ok(v) if !v.is_empty() => Ok(v),
        _ => Err(AuthError::MissingEnv(name)),
    }
}
