use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Where bidsmith keeps machine-global credentials. `BIDSMITH_HOME` overrides
/// (used by tests and by anyone who wants the file elsewhere); otherwise it is
/// `~/.bidsmith`.
pub fn bidsmith_home() -> PathBuf {
    if let Some(dir) = std::env::var_os("BIDSMITH_HOME") {
        return PathBuf::from(dir);
    }
    let base = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join(".bidsmith")
}

pub fn credentials_path() -> PathBuf {
    bidsmith_home().join("credentials.toml")
}

pub fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.is_empty())
}

/// The OAuth client bidsmith ships by default. Injected at build time once the
/// client has cleared Google's OAuth verification; absent (and harmless) in
/// ordinary builds, where only a bring-your-own client works.
pub fn default_client_id() -> Option<String> {
    option_env!("BIDSMITH_DEFAULT_CLIENT_ID")
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

pub fn default_client_secret() -> Option<String> {
    option_env!("BIDSMITH_DEFAULT_CLIENT_SECRET")
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// The persisted half of the credential set — everything that is not a CLI flag
/// or an environment variable. Written by `bidsmith auth login`.
#[derive(Default, Serialize, Deserialize)]
pub struct StoredCreds {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub developer_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login_customer_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub customer_id: Option<String>,
}

impl StoredCreds {
    pub fn load() -> Self {
        match std::fs::read_to_string(credentials_path()) {
            Ok(raw) => toml::from_str(&raw).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let home = bidsmith_home();
        std::fs::create_dir_all(&home)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700));
        }
        let raw = toml::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        crate::api::cache::write_atomic(&credentials_path(), raw.as_bytes(), 0o600)
    }
}

/// env var > stored file > built-in default.
fn choose(env_val: Option<String>, stored: Option<&String>, default: Option<String>) -> Option<String> {
    env_val.or_else(|| stored.cloned()).or(default)
}

/// A refresh token only works with the OAuth client that minted it. If the
/// active client_id no longer matches the one recorded at login (e.g. an env
/// override now points at a different client), the stored refresh token is
/// dead — say so instead of letting Google return an opaque `invalid_grant`.
fn detect_mismatch(
    refresh_from_env: bool,
    stored_refresh: Option<&str>,
    stored_client_id: Option<&str>,
    effective_client_id: Option<&str>,
) -> bool {
    if refresh_from_env {
        return false;
    }
    match (stored_refresh, stored_client_id, effective_client_id) {
        (Some(_), Some(sc), Some(ec)) => sc != ec,
        _ => false,
    }
}

/// The merged view of every credential, resolved on demand from
/// env vars, the stored file, and the built-in default client.
pub struct Resolved {
    pub stored: StoredCreds,
}

impl Resolved {
    pub fn load() -> Self {
        Self { stored: StoredCreds::load() }
    }

    pub fn client_id(&self) -> Option<String> {
        choose(
            env_nonempty("GOOGLE_ADS_CLIENT_ID"),
            self.stored.client_id.as_ref(),
            default_client_id(),
        )
    }

    pub fn client_secret(&self) -> Option<String> {
        let direct = env_nonempty("GOOGLE_ADS_CLIENT_SECRET")
            .or_else(|| self.stored.client_secret.clone());
        direct.or_else(|| {
            if self.client_id() == default_client_id() {
                default_client_secret()
            } else {
                None
            }
        })
    }

    pub fn refresh_token(&self) -> Option<String> {
        choose(
            env_nonempty("GOOGLE_ADS_REFRESH_TOKEN"),
            self.stored.refresh_token.as_ref(),
            None,
        )
    }

    pub fn developer_token(&self) -> Option<String> {
        choose(
            env_nonempty("GOOGLE_ADS_DEVELOPER_TOKEN"),
            self.stored.developer_token.as_ref(),
            None,
        )
    }

    pub fn login_customer_id(&self) -> Option<String> {
        choose(
            env_nonempty("GOOGLE_ADS_LOGIN_CUSTOMER_ID"),
            self.stored.login_customer_id.as_ref(),
            None,
        )
    }

    pub fn customer_id(&self) -> Option<String> {
        choose(
            env_nonempty("GOOGLE_ADS_CUSTOMER_ID"),
            self.stored.customer_id.as_ref(),
            None,
        )
    }

    pub fn client_mismatch(&self) -> Option<String> {
        let mismatch = detect_mismatch(
            env_nonempty("GOOGLE_ADS_REFRESH_TOKEN").is_some(),
            self.stored.refresh_token.as_deref(),
            self.stored.client_id.as_deref(),
            self.client_id().as_deref(),
        );
        mismatch.then(|| {
            "the saved Google sign-in was created with a different OAuth client than the one \
             now in effect — run `bidsmith auth login` again, or unset GOOGLE_ADS_CLIENT_ID."
                .to_string()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choose_precedence() {
        let stored = "stored".to_string();
        assert_eq!(
            choose(Some("env".into()), Some(&stored), Some("default".into())).as_deref(),
            Some("env"),
        );
        assert_eq!(
            choose(None, Some(&stored), Some("default".into())).as_deref(),
            Some("stored"),
        );
        assert_eq!(
            choose(None, None, Some("default".into())).as_deref(),
            Some("default"),
        );
        assert_eq!(choose(None, None, None), None);
    }

    #[test]
    fn mismatch_only_when_stored_token_outlives_its_client() {
        assert!(detect_mismatch(false, Some("rt"), Some("clientA"), Some("clientB")));
        assert!(!detect_mismatch(false, Some("rt"), Some("clientA"), Some("clientA")));
        assert!(!detect_mismatch(true, Some("rt"), Some("clientA"), Some("clientB")));
        assert!(!detect_mismatch(false, None, Some("clientA"), Some("clientB")));
        assert!(!detect_mismatch(false, Some("rt"), None, Some("clientB")));
    }

    #[test]
    fn stored_creds_toml_round_trip() {
        let creds = StoredCreds {
            client_id: Some("id".into()),
            client_secret: None,
            refresh_token: Some("rt".into()),
            developer_token: Some("dev".into()),
            login_customer_id: Some("1234567890".into()),
            customer_id: None,
        };
        let raw = toml::to_string_pretty(&creds).unwrap();
        assert!(!raw.contains("client_secret"), "None fields are skipped");
        let back: StoredCreds = toml::from_str(&raw).unwrap();
        assert_eq!(back.client_id.as_deref(), Some("id"));
        assert_eq!(back.refresh_token.as_deref(), Some("rt"));
        assert_eq!(back.client_secret, None);
        assert_eq!(back.customer_id, None);
    }
}
