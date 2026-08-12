//! The API's own answer to "what fields does this resource have, and which of
//! them can a `.bid` set?".
//!
//! Two published sources, each authoritative for one half of the question:
//!
//! * `GoogleAdsFieldService` lists the field paths a GAQL `SELECT` may name.
//!   It is the only source that gets the boundaries right — `campaign.manual_cpv`
//!   is a whole (empty) message you select as one field, while
//!   `campaign.network_settings` is not selectable at all, only its leaves are.
//! * The discovery document flags every output-only field `readOnly`, which
//!   `GoogleAdsFieldService` does not report. A metric, a `primary_status`, or
//!   an `effective_*` mirror is not something a repo could declare, so counting
//!   it as an unmodelled field would bury the ones that matter under noise.
//!
//! Both are fetched live rather than baked into the binary: a stale catalog
//! would under-report exactly the way issue #111 is about, and Google adds
//! fields inside a version.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::cache;
use crate::api::client::{self, ApiError, Client};

const CATALOG_FILE: &str = "field-catalog.json";
/// Fields are added inside an API version, so the catalog expires — but slowly:
/// it describes the API, not the account, and refetching it costs ~20 calls.
const CATALOG_TTL_SECS: u64 = 7 * 24 * 3600;
/// Deep enough for the deepest real nesting
/// (`campaign.video_campaign_settings.video_ad_format_control.…`), shallow
/// enough that a self-referential message can't run away.
const MAX_DEPTH: usize = 10;

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("API error: {0}")]
    Api(#[from] ApiError),
    #[error("googleAdsFields:search returned HTTP {status}: {body}")]
    Http { status: u16, body: String },
    #[error("the {0} discovery document has no `schemas` object")]
    MalformedDiscovery(String),
}

/// One field the API exposes on a resource.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CatalogField {
    /// The GAQL path, e.g. `campaign.geo_target_type_setting.positive_geo_target_type`.
    pub name: String,
    /// Whether a mutate could write it. Output-only fields are still listed so
    /// the count of what bidsmith *could* model stays honest.
    pub settable: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct FieldCatalog {
    /// Resource name (`campaign`, `ad_group`, …) -> its selectable attributes.
    pub by_resource: BTreeMap<String, Vec<CatalogField>>,
}

impl FieldCatalog {
    /// The fields on `resource` a `.bid` could in principle declare: selectable
    /// (so `plan` could read them) and settable (so `apply` could write them).
    pub fn settable_fields(&self, resource: &str) -> Vec<&str> {
        self.by_resource
            .get(resource)
            .map(|fields| {
                fields
                    .iter()
                    .filter(|f| f.settable)
                    .map(|f| f.name.as_str())
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[derive(Serialize, Deserialize)]
struct CatalogCache {
    api_version: String,
    resources_fingerprint: String,
    fetched_at: u64,
    catalog: FieldCatalog,
}

/// How the on-disk catalog cache is consulted.
pub enum CacheMode {
    /// Read if fresh, else fetch and write.
    ReadWrite,
    /// Always refetch; still write the result back.
    RefreshWrite,
}

/// Load the field catalog for `resources`, from cache when it is fresh.
pub fn load(
    client: &Client,
    access_token: &str,
    resources: &[&str],
    mode: CacheMode,
    label: &str,
) -> Result<FieldCatalog, CatalogError> {
    let api_version = client::api_version();
    let fingerprint = cache::fingerprint(&resources.join(","));
    let cache_dir = cache::project_cache_dir();
    let env_off = cache::disabled_by_env();

    if matches!(mode, CacheMode::ReadWrite) && !env_off {
        if let Some(hit) = load_cached(&cache_dir, &api_version, &fingerprint) {
            return Ok(hit);
        }
    }

    eprintln!("{label}: fetching the {api_version} field catalog...");
    let discovery = client::fetch_discovery_document()?;
    let settable = settable_paths(&discovery, resources, &api_version)?;

    let mut by_resource: BTreeMap<String, Vec<CatalogField>> = BTreeMap::new();
    for resource in resources {
        let mut fields: Vec<CatalogField> = selectable_attributes(client, access_token, resource)?
            .into_iter()
            .map(|name| {
                let settable = settable.contains(name.as_str());
                CatalogField { name, settable }
            })
            .collect();
        fields.sort_by(|a, b| a.name.cmp(&b.name));
        by_resource.insert((*resource).to_string(), fields);
    }

    let catalog = FieldCatalog { by_resource };
    if !env_off {
        let _ = save_cached(&cache_dir, &api_version, &fingerprint, &catalog);
    }
    Ok(catalog)
}

fn load_cached(
    cache_dir: &std::path::Path,
    api_version: &str,
    fingerprint: &str,
) -> Option<FieldCatalog> {
    let raw = std::fs::read_to_string(cache_dir.join(CATALOG_FILE)).ok()?;
    let cached: CatalogCache = serde_json::from_str(&raw).ok()?;
    if cached.api_version != api_version || cached.resources_fingerprint != fingerprint {
        return None;
    }
    if cache::now_unix().saturating_sub(cached.fetched_at) > CATALOG_TTL_SECS {
        return None;
    }
    Some(cached.catalog)
}

fn save_cached(
    cache_dir: &std::path::Path,
    api_version: &str,
    fingerprint: &str,
    catalog: &FieldCatalog,
) -> std::io::Result<()> {
    std::fs::create_dir_all(cache_dir)?;
    let cached = CatalogCache {
        api_version: api_version.to_string(),
        resources_fingerprint: fingerprint.to_string(),
        fetched_at: cache::now_unix(),
        catalog: FieldCatalog {
            by_resource: catalog.by_resource.clone(),
        },
    };
    let raw = serde_json::to_string(&cached).map_err(std::io::Error::other)?;
    cache::write_atomic(&cache_dir.join(CATALOG_FILE), raw.as_bytes(), 0o644)
}

/// Every attribute of `resource` a GAQL `SELECT` accepts.
fn selectable_attributes(
    client: &Client,
    access_token: &str,
    resource: &str,
) -> Result<Vec<String>, CatalogError> {
    let query =
        format!("SELECT name, category, selectable WHERE name LIKE '{resource}.%'");
    let mut names: Vec<String> = Vec::new();
    let mut page_token: Option<String> = None;
    loop {
        let response =
            client.search_google_ads_fields(access_token, &query, page_token.as_deref())?;
        if response.status < 200 || response.status >= 300 {
            return Err(CatalogError::Http {
                status: response.status,
                body: response.body_raw,
            });
        }
        for field in response
            .body
            .get("results")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let selectable = field.get("selectable").and_then(Value::as_bool) == Some(true);
            let is_attribute =
                field.get("category").and_then(Value::as_str) == Some("ATTRIBUTE");
            if !selectable || !is_attribute {
                continue;
            }
            if let Some(name) = field.get("name").and_then(Value::as_str) {
                names.push(name.to_string());
            }
        }
        match response.body.get("nextPageToken").and_then(Value::as_str) {
            Some(token) if !token.is_empty() => page_token = Some(token.to_string()),
            _ => break,
        }
    }
    Ok(names)
}

/// The GAQL paths under `resources` that a mutate could write, derived from the
/// discovery document's `readOnly` flags.
///
/// The walk emits a path for every node, not only for leaves: `SELECT` may name
/// a message field whole (`campaign.frequency_caps`,
/// `ad_group_ad.ad.demand_gen_video_responsive_ad.business_name`), and it is
/// `GoogleAdsFieldService`, not this walk, that decides which form is legal.
fn settable_paths(
    discovery: &Value,
    resources: &[&str],
    api_version: &str,
) -> Result<BTreeSet<String>, CatalogError> {
    let schemas = discovery
        .get("schemas")
        .and_then(Value::as_object)
        .ok_or_else(|| CatalogError::MalformedDiscovery(api_version.to_string()))?;

    let mut out: BTreeSet<String> = BTreeSet::new();
    for resource in resources {
        let suffix = format!("Resources__{}", pascal_case(resource));
        let Some(schema_id) = schemas.keys().find(|k| k.ends_with(&suffix)) else {
            continue;
        };
        let mut seen: BTreeSet<String> = BTreeSet::new();
        seen.insert(schema_id.clone());
        walk(schemas, schema_id, resource, &mut seen, 0, &mut out);
    }
    Ok(out)
}

fn walk(
    schemas: &serde_json::Map<String, Value>,
    schema_id: &str,
    prefix: &str,
    seen: &mut BTreeSet<String>,
    depth: usize,
    out: &mut BTreeSet<String>,
) {
    let Some(properties) = schemas
        .get(schema_id)
        .and_then(|s| s.get("properties"))
        .and_then(Value::as_object)
    else {
        return;
    };
    for (property, spec) in properties {
        if spec.get("readOnly").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        // A `resource_name` names the thing, it does not configure it. The
        // discovery document leaves it writable because that is how a mutate
        // addresses an existing resource, so nothing but this rules it out —
        // and left in, it is the single most-set "unmodelled field" on the
        // account, burying the gaps that are real ones (issue #136).
        if property == "resourceName" {
            continue;
        }
        let path = format!("{prefix}.{}", snake_case(property));
        out.insert(path.clone());
        let Some(child) = spec.get("$ref").and_then(Value::as_str) else {
            continue;
        };
        if depth >= MAX_DEPTH || !seen.insert(child.to_string()) {
            continue;
        }
        walk(schemas, child, &path, seen, depth + 1, out);
        seen.remove(child);
    }
}

/// `ad_group_ad` -> `AdGroupAd`, matching the discovery document's schema ids.
fn pascal_case(snake: &str) -> String {
    snake
        .split('_')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// `cpcBidMicros` -> `cpc_bid_micros`, matching GAQL field paths. Digits never
/// start a new word, so `description1` and `path2` survive intact.
fn snake_case(camel: &str) -> String {
    let mut out = String::with_capacity(camel.len() + 4);
    for (i, ch) in camel.char_indices() {
        if ch.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snake_case_matches_gaql_field_paths() {
        assert_eq!(snake_case("cpcBidMicros"), "cpc_bid_micros");
        assert_eq!(snake_case("name"), "name");
        assert_eq!(snake_case("description1"), "description1");
        assert_eq!(snake_case("path2"), "path2");
        assert_eq!(snake_case("targetRoas"), "target_roas");
    }

    #[test]
    fn pascal_case_matches_discovery_schema_ids() {
        assert_eq!(pascal_case("ad_group_ad"), "AdGroupAd");
        assert_eq!(pascal_case("campaign"), "Campaign");
        assert_eq!(pascal_case("shared_criterion"), "SharedCriterion");
    }

    fn discovery_fixture() -> Value {
        serde_json::json!({
            "schemas": {
                "GoogleAdsGoogleadsV22Resources__Campaign": {
                    "properties": {
                        "name": { "type": "string" },
                        "id": { "type": "string", "readOnly": true },
                        "resourceName": { "type": "string" },
                        "primaryStatus": { "type": "string", "readOnly": true },
                        "networkSettings": {
                            "$ref": "GoogleAdsGoogleadsV22Common__NetworkSettings"
                        },
                        "aiMaxSetting": {
                            "$ref": "GoogleAdsGoogleadsV22Common__AiMaxSetting"
                        },
                    }
                },
                "GoogleAdsGoogleadsV22Common__NetworkSettings": {
                    "properties": {
                        "targetGoogleSearch": { "type": "boolean" },
                        "targetYoutube": { "type": "boolean" },
                    }
                },
                "GoogleAdsGoogleadsV22Common__AiMaxSetting": {
                    "properties": {
                        "enableAiMax": { "type": "boolean" },
                        "bundlingRequired": { "type": "boolean", "readOnly": true },
                    }
                },
            }
        })
    }

    #[test]
    fn output_only_fields_are_not_settable() {
        let paths = settable_paths(&discovery_fixture(), &["campaign"], "v22").unwrap();
        assert!(paths.contains("campaign.name"));
        assert!(paths.contains("campaign.network_settings.target_google_search"));
        assert!(
            !paths.contains("campaign.id"),
            "an output-only field is not something a .bid could declare",
        );
        assert!(!paths.contains("campaign.primary_status"));
    }

    #[test]
    fn output_only_leaves_are_dropped_under_a_settable_parent() {
        let paths = settable_paths(&discovery_fixture(), &["campaign"], "v22").unwrap();
        assert!(paths.contains("campaign.ai_max_setting.enable_ai_max"));
        assert!(
            !paths.contains("campaign.ai_max_setting.bundling_required"),
            "readOnly is per field, not per message",
        );
    }

    #[test]
    fn a_message_field_is_a_path_of_its_own() {
        // `SELECT` may name a message whole, so the walk has to offer both
        // forms and let GoogleAdsFieldService pick.
        let paths = settable_paths(&discovery_fixture(), &["campaign"], "v22").unwrap();
        assert!(paths.contains("campaign.network_settings"));
        assert!(paths.contains("campaign.network_settings.target_youtube"));
    }

    #[test]
    fn a_resource_name_is_an_identity_not_a_setting() {
        // Writable per the discovery document, because that is how a mutate
        // addresses an existing resource — but nothing in a `.bid` chooses it,
        // and left in it is the loudest "unmodelled field" on the account.
        let paths = settable_paths(&discovery_fixture(), &["campaign"], "v22").unwrap();
        assert!(!paths.contains("campaign.resource_name"));
        assert!(paths.contains("campaign.name"));
    }

    #[test]
    fn a_missing_resource_schema_is_skipped_not_fatal() {
        let paths =
            settable_paths(&discovery_fixture(), &["campaign", "no_such_resource"], "v22")
                .unwrap();
        assert!(paths.contains("campaign.name"));
        assert!(!paths.iter().any(|p| p.starts_with("no_such_resource.")));
    }

    #[test]
    fn a_discovery_document_without_schemas_is_an_error() {
        let err = settable_paths(&Value::Null, &["campaign"], "v22").unwrap_err();
        assert!(matches!(err, CatalogError::MalformedDiscovery(_)));
    }

    #[test]
    fn settable_fields_filters_out_output_only_entries() {
        let mut by_resource = BTreeMap::new();
        by_resource.insert(
            "campaign".to_string(),
            vec![
                CatalogField { name: "campaign.name".into(), settable: true },
                CatalogField { name: "campaign.id".into(), settable: false },
            ],
        );
        let catalog = FieldCatalog { by_resource };
        assert_eq!(catalog.settable_fields("campaign"), vec!["campaign.name"]);
        assert!(catalog.settable_fields("ad_group").is_empty());
    }
}
