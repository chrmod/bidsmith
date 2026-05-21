use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const CACHE_DIR: &str = ".bidsmith/cache";
pub const TOKEN_FILE: &str = "token.json";
pub const LIVE_STATE_FILE: &str = "live-state.json";
pub const DEFAULT_LIVE_STATE_TTL_SECS: u64 = 900;

pub fn disabled_by_env() -> bool {
    matches!(
        std::env::var("BIDSMITH_NO_CACHE").as_deref(),
        Ok(v) if !v.is_empty() && v != "0"
    )
}

pub fn project_cache_dir() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    cwd.join(CACHE_DIR)
}

pub fn live_state_ttl_secs() -> u64 {
    std::env::var("BIDSMITH_CACHE_TTL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_LIVE_STATE_TTL_SECS)
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn fingerprint(s: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

#[derive(Serialize, Deserialize)]
pub struct TokenCache {
    pub fingerprint: String,
    pub access_token: String,
    pub expires_at: u64,
}

pub fn load_token(cache_dir: &Path, fp: &str) -> Option<TokenCache> {
    let raw = std::fs::read_to_string(cache_dir.join(TOKEN_FILE)).ok()?;
    let cached: TokenCache = serde_json::from_str(&raw).ok()?;
    if cached.fingerprint != fp {
        return None;
    }
    if cached.expires_at <= now_unix().saturating_add(60) {
        return None;
    }
    Some(cached)
}

pub fn save_token(cache_dir: &Path, cached: &TokenCache) -> std::io::Result<()> {
    std::fs::create_dir_all(cache_dir)?;
    let raw = serde_json::to_string_pretty(cached)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    write_atomic(&cache_dir.join(TOKEN_FILE), raw.as_bytes(), 0o600)
}

#[derive(Serialize, Deserialize)]
pub struct LiveStateCache {
    pub customer_id: String,
    #[serde(default)]
    pub login_customer_id: Option<String>,
    pub api_version: String,
    pub fetched_at: u64,
    pub batches: Vec<Value>,
}

pub struct LiveStateHit {
    pub batches: Vec<Value>,
    pub age_secs: u64,
}

pub fn load_live_state(
    cache_dir: &Path,
    customer_id: &str,
    login_customer_id: Option<&str>,
    api_version: &str,
    ttl_secs: u64,
) -> Option<LiveStateHit> {
    let raw = std::fs::read_to_string(cache_dir.join(LIVE_STATE_FILE)).ok()?;
    let cached: LiveStateCache = serde_json::from_str(&raw).ok()?;
    if cached.customer_id != customer_id
        || cached.login_customer_id.as_deref() != login_customer_id
        || cached.api_version != api_version
    {
        return None;
    }
    let age = now_unix().saturating_sub(cached.fetched_at);
    if age > ttl_secs {
        return None;
    }
    Some(LiveStateHit {
        batches: cached.batches,
        age_secs: age,
    })
}

pub fn save_live_state(
    cache_dir: &Path,
    customer_id: &str,
    login_customer_id: Option<&str>,
    api_version: &str,
    batches: &[Value],
) -> std::io::Result<()> {
    std::fs::create_dir_all(cache_dir)?;
    let cached = LiveStateCache {
        customer_id: customer_id.to_string(),
        login_customer_id: login_customer_id.map(|s| s.to_string()),
        api_version: api_version.to_string(),
        fetched_at: now_unix(),
        batches: batches.to_vec(),
    };
    let raw = serde_json::to_string(&cached)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    write_atomic(&cache_dir.join(LIVE_STATE_FILE), raw.as_bytes(), 0o644)
}

pub fn invalidate_live_state(cache_dir: &Path) {
    let _ = std::fs::remove_file(cache_dir.join(LIVE_STATE_FILE));
}

fn write_atomic(path: &Path, data: &[u8], _mode: u32) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("cache");
    let tmp = parent.join(format!(".{name}.tmp.{}", std::process::id()));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(_mode))?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn format_age(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bidsmith-cache-test-{name}-{}-{}",
            std::process::id(),
            now_unix(),
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn token_round_trip_and_fingerprint_mismatch() {
        let dir = tmp_dir("token-rt");
        let token = TokenCache {
            fingerprint: "deadbeefdeadbeef".to_string(),
            access_token: "ya29.fake".to_string(),
            expires_at: now_unix() + 3600,
        };
        save_token(&dir, &token).unwrap();

        let same = load_token(&dir, "deadbeefdeadbeef").unwrap();
        assert_eq!(same.access_token, "ya29.fake");

        assert!(
            load_token(&dir, "0000000000000000").is_none(),
            "fingerprint mismatch must invalidate",
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn token_expiry_skews_cache_miss() {
        let dir = tmp_dir("token-exp");
        let token = TokenCache {
            fingerprint: "f".to_string(),
            access_token: "expired".to_string(),
            expires_at: now_unix() + 30,
        };
        save_token(&dir, &token).unwrap();
        assert!(
            load_token(&dir, "f").is_none(),
            "tokens within 60s of expiry should not be reused",
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn live_state_round_trip_and_ttl() {
        let dir = tmp_dir("live");
        let batches = vec![serde_json::json!({"results": [{"campaign": {"id": "1"}}]})];
        save_live_state(&dir, "123", Some("999"), "v22", &batches).unwrap();

        let hit = load_live_state(&dir, "123", Some("999"), "v22", 900).unwrap();
        assert_eq!(hit.batches, batches);

        assert!(load_live_state(&dir, "OTHER", Some("999"), "v22", 900).is_none());
        assert!(load_live_state(&dir, "123", None, "v22", 900).is_none());
        assert!(load_live_state(&dir, "123", Some("999"), "v23", 900).is_none());

        let aged = LiveStateCache {
            customer_id: "123".into(),
            login_customer_id: Some("999".into()),
            api_version: "v22".into(),
            fetched_at: now_unix().saturating_sub(10_000),
            batches: batches.clone(),
        };
        std::fs::write(
            dir.join(LIVE_STATE_FILE),
            serde_json::to_string(&aged).unwrap(),
        )
        .unwrap();
        assert!(
            load_live_state(&dir, "123", Some("999"), "v22", 900).is_none(),
            "entries older than the TTL must miss",
        );

        invalidate_live_state(&dir);
        assert!(load_live_state(&dir, "123", Some("999"), "v22", 900).is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fingerprint_is_deterministic_and_distinct() {
        assert_eq!(fingerprint("hello"), fingerprint("hello"));
        assert_ne!(fingerprint("hello"), fingerprint("world"));
        assert_eq!(fingerprint("").len(), 16);
    }
}
