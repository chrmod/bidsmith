use serde_json::Value;

use crate::api::cache;
use crate::api::client::{self, ApiError, Client};
use crate::commands::adapt;
use crate::commands::export::ExportInput;

#[derive(Debug, thiserror::Error)]
pub enum LiveStateError {
    #[error("API error: {0}")]
    Api(#[from] ApiError),
    #[error("query {query_label} returned HTTP {status}: {body}")]
    Http {
        query_label: &'static str,
        status: u16,
        body: String,
    },
    #[error("query {query_label} returned an unparseable {status} response: {body}")]
    MalformedResponse {
        query_label: &'static str,
        status: u16,
        body: String,
    },
    #[error("adapter error: {0}")]
    Adapter(String),
}

pub const QUERIES: &[(&str, &str)] = &[
    (
        "campaign_budget",
        "SELECT
          campaign_budget.resource_name,
          campaign_budget.id,
          campaign_budget.name,
          campaign_budget.amount_micros,
          campaign_budget.delivery_method,
          campaign_budget.explicitly_shared
        FROM campaign_budget",
    ),
    (
        "campaign",
        "SELECT
          campaign.resource_name,
          campaign.id,
          campaign.name,
          campaign.status,
          campaign.advertising_channel_type,
          campaign.campaign_budget,
          campaign.contains_eu_political_advertising,
          campaign.manual_cpc.enhanced_cpc_enabled,
          campaign.network_settings.target_google_search,
          campaign.network_settings.target_search_network,
          campaign.network_settings.target_content_network,
          campaign.network_settings.target_partner_search_network
        FROM campaign
        WHERE campaign.status != 'REMOVED'",
    ),
    (
        "ad_group",
        "SELECT
          ad_group.resource_name,
          ad_group.id,
          ad_group.name,
          ad_group.campaign,
          ad_group.status,
          ad_group.type,
          ad_group.cpc_bid_micros
        FROM ad_group
        WHERE ad_group.status != 'REMOVED'",
    ),
    (
        "ad_group_ad",
        "SELECT
          ad_group_ad.resource_name,
          ad_group_ad.ad_group,
          ad_group_ad.status,
          ad_group_ad.ad.id,
          ad_group_ad.ad.name,
          ad_group_ad.ad.final_urls,
          ad_group_ad.ad.responsive_search_ad.headlines,
          ad_group_ad.ad.responsive_search_ad.descriptions,
          ad_group_ad.ad.responsive_search_ad.path1,
          ad_group_ad.ad.responsive_search_ad.path2,
          ad_group_ad.ad.video_responsive_ad.headlines,
          ad_group_ad.ad.video_responsive_ad.long_headlines,
          ad_group_ad.ad.video_responsive_ad.descriptions,
          ad_group_ad.ad.video_responsive_ad.call_to_actions,
          ad_group_ad.ad.video_responsive_ad.videos,
          ad_group_ad.ad.demand_gen_video_responsive_ad.headlines,
          ad_group_ad.ad.demand_gen_video_responsive_ad.long_headlines,
          ad_group_ad.ad.demand_gen_video_responsive_ad.descriptions,
          ad_group_ad.ad.demand_gen_video_responsive_ad.call_to_actions,
          ad_group_ad.ad.demand_gen_video_responsive_ad.videos,
          ad_group_ad.ad.demand_gen_video_responsive_ad.breadcrumb1,
          ad_group_ad.ad.demand_gen_video_responsive_ad.breadcrumb2,
          ad_group_ad.ad.demand_gen_video_responsive_ad.business_name
        FROM ad_group_ad
        WHERE ad_group_ad.status != 'REMOVED'",
    ),
    (
        "ad_group_criterion",
        "SELECT
          ad_group_criterion.resource_name,
          ad_group_criterion.ad_group,
          ad_group_criterion.status,
          ad_group_criterion.negative,
          ad_group_criterion.cpc_bid_micros,
          ad_group_criterion.keyword.text,
          ad_group_criterion.keyword.match_type
        FROM ad_group_criterion
        WHERE ad_group_criterion.type = KEYWORD
          AND ad_group_criterion.status != 'REMOVED'",
    ),
    (
        "campaign_criterion",
        "SELECT
          campaign_criterion.resource_name,
          campaign_criterion.campaign,
          campaign_criterion.status,
          campaign_criterion.negative,
          campaign_criterion.bid_modifier,
          campaign_criterion.keyword.text,
          campaign_criterion.keyword.match_type,
          campaign_criterion.location.geo_target_constant,
          campaign_criterion.language.language_constant,
          campaign_criterion.proximity.geo_point.latitude_in_micro_degrees,
          campaign_criterion.proximity.geo_point.longitude_in_micro_degrees,
          campaign_criterion.proximity.radius,
          campaign_criterion.proximity.radius_units,
          campaign_criterion.device.type
        FROM campaign_criterion
        WHERE campaign_criterion.type IN (KEYWORD, LOCATION, LANGUAGE, PROXIMITY, DEVICE)
          AND campaign_criterion.status != 'REMOVED'",
    ),
    (
        "conversion_action",
        "SELECT
          conversion_action.resource_name,
          conversion_action.id,
          conversion_action.name,
          conversion_action.type,
          conversion_action.category,
          conversion_action.status,
          conversion_action.counting_type,
          conversion_action.click_through_lookback_window_days,
          conversion_action.view_through_lookback_window_days,
          conversion_action.value_settings.default_value,
          conversion_action.value_settings.default_currency_code,
          conversion_action.value_settings.always_use_default_value
        FROM conversion_action
        WHERE conversion_action.status != 'REMOVED'",
    ),
    (
        "call_asset",
        "SELECT
          asset.resource_name,
          asset.id,
          asset.call_asset.country_code,
          asset.call_asset.phone_number,
          asset.call_asset.call_conversion_reporting_state,
          asset.call_asset.call_conversion_action
        FROM asset
        WHERE asset.type = CALL",
    ),
    (
        "sitelink_asset",
        "SELECT
          asset.resource_name,
          asset.id,
          asset.final_urls,
          asset.sitelink_asset.link_text,
          asset.sitelink_asset.description1,
          asset.sitelink_asset.description2
        FROM asset
        WHERE asset.type = SITELINK",
    ),
    (
        "callout_asset",
        "SELECT
          asset.resource_name,
          asset.id,
          asset.callout_asset.callout_text
        FROM asset
        WHERE asset.type = CALLOUT",
    ),
    (
        "structured_snippet_asset",
        "SELECT
          asset.resource_name,
          asset.id,
          asset.structured_snippet_asset.header,
          asset.structured_snippet_asset.values
        FROM asset
        WHERE asset.type = STRUCTURED_SNIPPET",
    ),
    (
        "youtube_video_asset",
        "SELECT
          asset.resource_name,
          asset.id,
          asset.youtube_video_asset.youtube_video_id,
          asset.youtube_video_asset.youtube_video_title
        FROM asset
        WHERE asset.type = YOUTUBE_VIDEO",
    ),
    (
        "customer_asset",
        "SELECT
          customer_asset.resource_name,
          customer_asset.asset,
          customer_asset.field_type,
          customer_asset.status
        FROM customer_asset
        WHERE customer_asset.field_type = CALL
          AND customer_asset.status != 'REMOVED'",
    ),
    (
        "campaign_asset",
        "SELECT
          campaign_asset.resource_name,
          campaign_asset.campaign,
          campaign_asset.asset,
          campaign_asset.field_type,
          campaign_asset.status
        FROM campaign_asset
        WHERE campaign_asset.field_type IN (SITELINK, CALLOUT, STRUCTURED_SNIPPET)
          AND campaign_asset.status != 'REMOVED'",
    ),
    (
        "ad_group_asset",
        "SELECT
          ad_group_asset.resource_name,
          ad_group_asset.ad_group,
          ad_group_asset.asset,
          ad_group_asset.field_type,
          ad_group_asset.status
        FROM ad_group_asset
        WHERE ad_group_asset.field_type IN (SITELINK, CALLOUT, STRUCTURED_SNIPPET)
          AND ad_group_asset.status != 'REMOVED'",
    ),
    (
        "shared_set",
        "SELECT
          shared_set.resource_name,
          shared_set.id,
          shared_set.name,
          shared_set.type,
          shared_set.status
        FROM shared_set
        WHERE shared_set.status != 'REMOVED'",
    ),
    (
        "shared_criterion",
        "SELECT
          shared_criterion.resource_name,
          shared_criterion.shared_set,
          shared_criterion.criterion_id,
          shared_criterion.keyword.text,
          shared_criterion.keyword.match_type
        FROM shared_criterion
        WHERE shared_criterion.type = KEYWORD",
    ),
    (
        "campaign_shared_set",
        "SELECT
          campaign_shared_set.resource_name,
          campaign_shared_set.campaign,
          campaign_shared_set.shared_set,
          campaign_shared_set.status
        FROM campaign_shared_set
        WHERE campaign_shared_set.status != 'REMOVED'",
    ),
    (
        "label",
        // A removed label still answers this query but can no longer be
        // attached to anything ("Inactive labels cannot be applied."), so
        // reusing its resource_name sinks the whole atomic batch. Skipping it
        // here makes the mutate builder mint a fresh label under the same name.
        "SELECT label.resource_name, label.name
        FROM label
        WHERE label.name LIKE 'bidsmith:%'
          AND label.status != 'REMOVED'",
    ),
    (
        "campaign_label",
        "SELECT campaign_label.campaign, label.name
        FROM campaign_label
        WHERE label.name LIKE 'bidsmith:%'",
    ),
    (
        "ad_group_label",
        "SELECT ad_group_label.ad_group, label.name
        FROM ad_group_label
        WHERE label.name LIKE 'bidsmith:%'",
    ),
    (
        "ad_group_ad_label",
        "SELECT ad_group_ad_label.ad_group_ad, label.name
        FROM ad_group_ad_label
        WHERE label.name LIKE 'bidsmith:address=%'",
    ),
    (
        "ad_group_criterion_label",
        "SELECT ad_group_criterion_label.ad_group_criterion, label.name
        FROM ad_group_criterion_label
        WHERE label.name LIKE 'bidsmith:address=%'",
    ),
];

pub fn queries_fingerprint() -> String {
    let mut joined = String::new();
    for (label, query) in QUERIES {
        joined.push_str(label);
        joined.push('\n');
        joined.push_str(query);
        joined.push('\n');
    }
    cache::fingerprint(&joined)
}

pub fn fetch_raw(client: &Client, access_token: &str) -> Result<Vec<Value>, LiveStateError> {
    let mut all_batches: Vec<Value> = Vec::new();
    for (label, query) in QUERIES {
        let response = client.search_stream(access_token, query)?;
        if response.status < 200 || response.status >= 300 {
            return Err(LiveStateError::Http {
                query_label: label,
                status: response.status,
                body: response.body_raw,
            });
        }
        if let Some(arr) = response.body.as_array() {
            all_batches.extend(arr.iter().cloned());
        } else if response.body.is_object() {
            all_batches.push(response.body.clone());
        } else {
            return Err(LiveStateError::MalformedResponse {
                query_label: label,
                status: response.status,
                body: response.body_raw,
            });
        }
    }
    Ok(all_batches)
}

fn adapt_batches(batches: Vec<Value>) -> Result<ExportInput, LiveStateError> {
    let mega = Value::Array(batches).to_string();
    adapt::from_search_response(&mega).map_err(LiveStateError::Adapter)
}

/// How the cache should be consulted during a fetch.
pub enum CacheMode {
    /// Read cache if fresh, else fetch and write cache. Default for plan/apply.
    ReadWrite,
    /// Always fetch fresh; still write the result to cache. `--refresh-state`.
    RefreshWrite,
    /// Bypass cache entirely (no read, no write).
    Bypass,
}

pub struct FetchOutcome {
    pub state: ExportInput,
}

pub fn fetch_with_cache(
    client: &Client,
    access_token: &str,
    mode: CacheMode,
    label: &str,
) -> Result<FetchOutcome, LiveStateError> {
    let env_off = cache::disabled_by_env();
    let effective_mode = if env_off { CacheMode::Bypass } else { mode };
    let cache_dir = cache::project_cache_dir();
    let api_v = client::api_version();
    let queries_fp = queries_fingerprint();
    let login = client.login_customer_id.as_deref();

    if matches!(effective_mode, CacheMode::ReadWrite) {
        if let Some(hit) = cache::load_live_state(
            &cache_dir,
            &client.customer_id,
            login,
            &api_v,
            &queries_fp,
            cache::live_state_ttl_secs(),
        ) {
            eprintln!(
                "{label}: using cached live state from {} ago (--refresh-state to refetch).",
                cache::format_age(hit.age_secs),
            );
            let state = adapt_batches(hit.batches)?;
            return Ok(FetchOutcome { state });
        }
    }

    eprintln!(
        "{label}: fetching live state from customers/{}...",
        client.customer_id,
    );
    let batches = fetch_raw(client, access_token)?;

    if !matches!(effective_mode, CacheMode::Bypass) {
        let _ = cache::save_live_state(
            &cache_dir,
            &client.customer_id,
            login,
            &api_v,
            &queries_fp,
            &batches,
        );
    }

    let state = adapt_batches(batches)?;
    Ok(FetchOutcome { state })
}

pub fn invalidate_cache() {
    cache::invalidate_live_state(&cache::project_cache_dir());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_query_skips_removed_labels() {
        let (_, query) = QUERIES.iter().find(|(name, _)| *name == "label").unwrap();
        assert!(
            query.contains("label.status != 'REMOVED'"),
            "a removed label cannot be attached to anything; indexing one makes \
             the mutate builder reuse it and the whole batch is rejected"
        );
    }
}
