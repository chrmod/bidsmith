use serde_json::Value;

use crate::api::client::{ApiError, Client};
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
    #[error("adapter error: {0}")]
    Adapter(String),
}

const QUERIES: &[(&str, &str)] = &[
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
          ad_group_ad.ad.responsive_search_ad.path2
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
          campaign_criterion.keyword.text,
          campaign_criterion.keyword.match_type,
          campaign_criterion.location.geo_target_constant,
          campaign_criterion.language.language_constant,
          campaign_criterion.proximity.geo_point.latitude_in_micro_degrees,
          campaign_criterion.proximity.geo_point.longitude_in_micro_degrees,
          campaign_criterion.proximity.radius,
          campaign_criterion.proximity.radius_units
        FROM campaign_criterion
        WHERE campaign_criterion.type IN (KEYWORD, LOCATION, LANGUAGE, PROXIMITY)
          AND campaign_criterion.status != 'REMOVED'",
    ),
];

pub fn fetch(client: &Client, access_token: &str) -> Result<ExportInput, LiveStateError> {
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
        }
    }
    let mega = Value::Array(all_batches).to_string();
    adapt::from_search_response(&mega).map_err(LiveStateError::Adapter)
}
