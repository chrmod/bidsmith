use serde_json::Value;

const DEFAULT_API_VERSION: &str = "v22";
const USER_AGENT: &str = concat!("bidsmith/", env!("CARGO_PKG_VERSION"));

pub fn api_version() -> String {
    std::env::var("BIDSMITH_API_VERSION")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_API_VERSION.to_string())
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("missing env var {0} — see ads/README.md (rezolutnie) or run `python -m ads.oauth_flow`")]
    MissingEnv(&'static str),
    #[error("HTTP error talking to googleads.googleapis.com: {0}")]
    Http(#[from] reqwest::Error),
}

pub struct MutateResponse {
    pub status: u16,
    pub body: Value,
    pub body_raw: String,
}

pub struct Client {
    http: reqwest::blocking::Client,
    pub customer_id: String,
    pub login_customer_id: Option<String>,
    pub developer_token: String,
}

impl Client {
    pub fn from_env() -> Result<Self, ApiError> {
        let customer_id = require_env("GOOGLE_ADS_CUSTOMER_ID")?;
        let developer_token = require_env("GOOGLE_ADS_DEVELOPER_TOKEN")?;
        let login_customer_id = std::env::var("GOOGLE_ADS_LOGIN_CUSTOMER_ID")
            .ok()
            .filter(|s| !s.is_empty());
        Ok(Self {
            http: reqwest::blocking::Client::builder()
                .user_agent(USER_AGENT)
                .build()?,
            customer_id,
            login_customer_id,
            developer_token,
        })
    }

    #[allow(dead_code)]
    pub fn googleads_mutate(
        &self,
        access_token: &str,
        body: &Value,
    ) -> Result<MutateResponse, ApiError> {
        self.post_json(access_token, "googleAds:mutate", body)
    }

    pub fn search_stream(
        &self,
        access_token: &str,
        query: &str,
    ) -> Result<MutateResponse, ApiError> {
        let body = serde_json::json!({ "query": query });
        self.post_json(access_token, "googleAds:searchStream", &body)
    }

    fn post_json(
        &self,
        access_token: &str,
        endpoint: &str,
        body: &Value,
    ) -> Result<MutateResponse, ApiError> {
        let version = api_version();
        let url = format!(
            "https://googleads.googleapis.com/{version}/customers/{}/{endpoint}",
            self.customer_id,
        );
        let mut req = self
            .http
            .post(&url)
            .bearer_auth(access_token)
            .header("developer-token", &self.developer_token);
        if let Some(login) = &self.login_customer_id {
            req = req.header("login-customer-id", login);
        }
        let response = req.json(body).send()?;
        let status = response.status().as_u16();
        let raw = response.text()?;
        let parsed = serde_json::from_str(&raw).unwrap_or(Value::String(raw.clone()));
        Ok(MutateResponse {
            status,
            body: parsed,
            body_raw: raw,
        })
    }
}

fn require_env(name: &'static str) -> Result<String, ApiError> {
    match std::env::var(name) {
        Ok(v) if !v.is_empty() => Ok(v),
        _ => Err(ApiError::MissingEnv(name)),
    }
}
