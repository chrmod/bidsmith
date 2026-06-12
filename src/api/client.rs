use serde_json::Value;

use crate::api::creds;

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
    #[error("missing {0}. Run `bidsmith auth login`, or set the matching GOOGLE_ADS_* env var.")]
    MissingCred(&'static str),
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
        let resolved = creds::Resolved::load();
        let customer_id = resolved.customer_id().ok_or(ApiError::MissingCred(
            "customer id (provider block, bidsmith.toml, or GOOGLE_ADS_CUSTOMER_ID)",
        ))?;
        Self::build(customer_id, resolved.login_customer_id())
    }

    /// Build a client aimed at an explicitly resolved target, used by `plan` /
    /// `apply` where the customer/login ids come from the `.bid` provider block,
    /// `bidsmith.toml`, or the environment (already merged by the importer).
    pub fn for_target(
        customer_id: &str,
        login_customer_id: Option<&str>,
    ) -> Result<Self, ApiError> {
        if customer_id.is_empty() {
            return Err(ApiError::MissingCred(
                "customer id (provider block, bidsmith.toml, or GOOGLE_ADS_CUSTOMER_ID)",
            ));
        }
        Self::build(customer_id.to_string(), login_customer_id.map(str::to_string))
    }

    fn build(customer_id: String, login_customer_id: Option<String>) -> Result<Self, ApiError> {
        let developer_token = creds::Resolved::load().developer_token().ok_or(
            ApiError::MissingCred("developer token (bidsmith.toml or GOOGLE_ADS_DEVELOPER_TOKEN)"),
        )?;
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

/// `customers:listAccessibleCustomers` — the accounts the signed-in user can
/// reach. Needs only the developer token + access token (no customer id, no
/// login-customer-id), which makes it the ideal post-login verification call.
/// Returns bare 10-digit customer ids (the `customers/` prefix stripped).
pub fn list_accessible_customers(
    developer_token: &str,
    access_token: &str,
) -> Result<Vec<String>, ApiError> {
    let version = api_version();
    let url =
        format!("https://googleads.googleapis.com/{version}/customers:listAccessibleCustomers");
    let http = reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .build()?;
    let response = http
        .get(&url)
        .bearer_auth(access_token)
        .header("developer-token", developer_token)
        .send()?;
    let raw = response.text()?;
    let parsed: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
    let names = parsed
        .get("resourceNames")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(|s| s.trim_start_matches("customers/").to_string())
                .collect()
        })
        .unwrap_or_default();
    Ok(names)
}
