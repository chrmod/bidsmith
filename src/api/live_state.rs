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
        // The currency every `amount_micros` in the account is denominated in,
        // so plan can report committed daily spend as money rather than micros.
        "customer",
        "SELECT
          customer.resource_name,
          customer.id,
          customer.currency_code
        FROM customer",
    ),
    (
        "campaign_budget",
        "SELECT
          campaign_budget.resource_name,
          campaign_budget.id,
          campaign_budget.name,
          campaign_budget.amount_micros,
          campaign_budget.total_amount_micros,
          campaign_budget.period,
          campaign_budget.type,
          campaign_budget.delivery_method,
          campaign_budget.explicitly_shared,
          campaign_budget.status
        FROM campaign_budget
        WHERE campaign_budget.status != 'REMOVED'",
    ),
    (
        "campaign",
        "SELECT
          campaign.resource_name,
          campaign.id,
          campaign.name,
          campaign.status,
          campaign.advertising_channel_type,
          campaign.advertising_channel_sub_type,
          campaign.start_date_time,
          campaign.end_date_time,
          campaign.campaign_budget,
          campaign.contains_eu_political_advertising,
          campaign.bidding_strategy_type,
          campaign.manual_cpc.enhanced_cpc_enabled,
          campaign.target_impression_share.location,
          campaign.target_impression_share.location_fraction_micros,
          campaign.target_impression_share.cpc_bid_ceiling_micros,
          campaign.target_spend.cpc_bid_ceiling_micros,
          campaign.network_settings.target_google_search,
          campaign.network_settings.target_search_network,
          campaign.network_settings.target_content_network,
          campaign.network_settings.target_partner_search_network,
          campaign.network_settings.target_youtube,
          campaign.network_settings.target_google_tv_network,
          campaign.geo_target_type_setting.positive_geo_target_type,
          campaign.geo_target_type_setting.negative_geo_target_type,
          campaign.video_campaign_settings.video_ad_inventory_control.allow_in_stream,
          campaign.video_campaign_settings.video_ad_inventory_control.allow_in_feed,
          campaign.video_campaign_settings.video_ad_inventory_control.allow_shorts,
          campaign.video_campaign_settings.video_ad_inventory_control.allow_non_skippable_in_stream,
          campaign.asset_automation_settings,
          campaign.ai_max_setting.enable_ai_max,
          campaign.demand_gen_campaign_settings.upgraded_targeting,
          campaign.dynamic_search_ads_setting.domain_name,
          campaign.dynamic_search_ads_setting.language_code,
          campaign.dynamic_search_ads_setting.use_supplied_urls_only,
          campaign.targeting_setting.target_restrictions,
          campaign.frequency_caps,
          campaign.final_url_suffix,
          campaign.url_custom_parameters
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
          ad_group.cpc_bid_micros,
          ad_group.cpv_bid_micros,
          ad_group.cpm_bid_micros,
          ad_group.target_cpa_micros,
          ad_group.target_cpm_micros,
          ad_group.target_cpv_micros,
          ad_group.percent_cpc_bid_micros,
          ad_group.fixed_cpm_micros,
          ad_group.targeting_setting.target_restrictions,
          ad_group.ai_max_ad_group_setting.disable_search_term_matching,
          ad_group.audience_setting.use_audience_grouped,
          ad_group.demand_gen_ad_group_settings.channel_controls.channel_config,
          ad_group.demand_gen_ad_group_settings.channel_controls.channel_strategy,
          ad_group.demand_gen_ad_group_settings.channel_controls.selected_channels.youtube_in_stream,
          ad_group.demand_gen_ad_group_settings.channel_controls.selected_channels.youtube_in_feed,
          ad_group.demand_gen_ad_group_settings.channel_controls.selected_channels.youtube_shorts,
          ad_group.demand_gen_ad_group_settings.channel_controls.selected_channels.gmail,
          ad_group.demand_gen_ad_group_settings.channel_controls.selected_channels.discover,
          ad_group.demand_gen_ad_group_settings.channel_controls.selected_channels.display,
          ad_group.demand_gen_ad_group_settings.channel_controls.selected_channels.maps,
          ad_group.final_url_suffix,
          ad_group.url_custom_parameters
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
          ad_group_ad.ad.final_mobile_urls,
          ad_group_ad.ad.display_url,
          ad_group_ad.ad.final_url_suffix,
          ad_group_ad.ad.url_custom_parameters,
          ad_group_ad.ad.responsive_search_ad.headlines,
          ad_group_ad.ad.responsive_search_ad.descriptions,
          ad_group_ad.ad.responsive_search_ad.path1,
          ad_group_ad.ad.responsive_search_ad.path2,
          ad_group_ad.ad.video_ad.video.asset,
          ad_group_ad.ad.video_responsive_ad.headlines,
          ad_group_ad.ad.video_responsive_ad.long_headlines,
          ad_group_ad.ad.video_responsive_ad.descriptions,
          ad_group_ad.ad.video_responsive_ad.call_to_actions,
          ad_group_ad.ad.video_responsive_ad.videos,
          ad_group_ad.ad.video_responsive_ad.breadcrumb1,
          ad_group_ad.ad.video_responsive_ad.breadcrumb2,
          ad_group_ad.ad.demand_gen_video_responsive_ad.headlines,
          ad_group_ad.ad.demand_gen_video_responsive_ad.long_headlines,
          ad_group_ad.ad.demand_gen_video_responsive_ad.descriptions,
          ad_group_ad.ad.demand_gen_video_responsive_ad.call_to_actions,
          ad_group_ad.ad.demand_gen_video_responsive_ad.videos,
          ad_group_ad.ad.demand_gen_video_responsive_ad.logo_images,
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
          ad_group_criterion.bid_modifier,
          ad_group_criterion.keyword.text,
          ad_group_criterion.keyword.match_type,
          ad_group_criterion.location.geo_target_constant,
          ad_group_criterion.language.language_constant,
          ad_group_criterion.youtube_channel.channel_id,
          ad_group_criterion.youtube_video.video_id,
          ad_group_criterion.topic.topic_constant,
          ad_group_criterion.placement.url,
          ad_group_criterion.user_interest.user_interest_category,
          ad_group_criterion.age_range.type,
          ad_group_criterion.gender.type,
          ad_group_criterion.parental_status.type,
          ad_group_criterion.income_range.type,
          ad_group_criterion.custom_audience.custom_audience,
          ad_group_criterion.user_list.user_list,
          ad_group_criterion.combined_audience.combined_audience,
          ad_group_criterion.audience.audience
        FROM ad_group_criterion
        WHERE ad_group_criterion.type IN (
            KEYWORD, LOCATION, LANGUAGE, YOUTUBE_CHANNEL, YOUTUBE_VIDEO, TOPIC,
            PLACEMENT, USER_INTEREST, AGE_RANGE, GENDER, PARENTAL_STATUS,
            INCOME_RANGE, CUSTOM_AUDIENCE, USER_LIST, COMBINED_AUDIENCE, AUDIENCE
          )
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
          campaign_criterion.ad_schedule.day_of_week,
          campaign_criterion.ad_schedule.start_hour,
          campaign_criterion.ad_schedule.start_minute,
          campaign_criterion.ad_schedule.end_hour,
          campaign_criterion.ad_schedule.end_minute,
          campaign_criterion.device.type,
          campaign_criterion.youtube_channel.channel_id,
          campaign_criterion.youtube_video.video_id,
          campaign_criterion.topic.topic_constant,
          campaign_criterion.user_interest.user_interest_category,
          campaign_criterion.age_range.type,
          campaign_criterion.gender.type,
          campaign_criterion.custom_audience.custom_audience,
          campaign_criterion.user_list.user_list,
          campaign_criterion.combined_audience.combined_audience
        FROM campaign_criterion
        WHERE campaign_criterion.type IN (
            KEYWORD, LOCATION, LANGUAGE, PROXIMITY, AD_SCHEDULE, DEVICE,
            YOUTUBE_CHANNEL, YOUTUBE_VIDEO, TOPIC, USER_INTEREST,
            AGE_RANGE, GENDER, CUSTOM_AUDIENCE, USER_LIST, COMBINED_AUDIENCE
          )
          AND campaign_criterion.status != 'REMOVED'",
    ),
    (
        // `audience.status` is output-only and the API has no remove operation
        // for an audience, so a removed one can only be filtered out here.
        "audience",
        "SELECT
          audience.resource_name,
          audience.id,
          audience.name,
          audience.description,
          audience.dimensions,
          audience.exclusion_dimension
        FROM audience
        WHERE audience.status != 'REMOVED'",
    ),
    (
        "custom_audience",
        "SELECT
          custom_audience.resource_name,
          custom_audience.id,
          custom_audience.name,
          custom_audience.description,
          custom_audience.type,
          custom_audience.status,
          custom_audience.members
        FROM custom_audience
        WHERE custom_audience.status != 'REMOVED'",
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
          conversion_action.primary_for_goal,
          conversion_action.include_in_conversions_metric,
          conversion_action.click_through_lookback_window_days,
          conversion_action.view_through_lookback_window_days,
          conversion_action.phone_call_duration_seconds,
          conversion_action.value_settings.default_value,
          conversion_action.value_settings.default_currency_code,
          conversion_action.value_settings.always_use_default_value,
          conversion_action.attribution_model_settings.attribution_model
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
        // `asset.name` is the whole payload a `.bid` can declare: the image
        // bytes are mutate-only and everything else the API reports about an
        // image is output-only.
        "image_asset",
        "SELECT
          asset.resource_name,
          asset.id,
          asset.name,
          asset.type
        FROM asset
        WHERE asset.type = IMAGE",
    ),
    (
        "call_to_action_asset",
        "SELECT
          asset.resource_name,
          asset.id,
          asset.call_to_action_asset.call_to_action
        FROM asset
        WHERE asset.type = CALL_TO_ACTION",
    ),
    (
        "customer_asset",
        "SELECT
          customer_asset.resource_name,
          customer_asset.asset,
          customer_asset.field_type,
          customer_asset.source,
          customer_asset.status
        FROM customer_asset
        WHERE customer_asset.status != 'REMOVED'",
    ),
    (
        "campaign_asset",
        "SELECT
          campaign_asset.resource_name,
          campaign_asset.campaign,
          campaign_asset.asset,
          campaign_asset.field_type,
          campaign_asset.source,
          campaign_asset.status
        FROM campaign_asset
        WHERE campaign_asset.status != 'REMOVED'",
    ),
    (
        "ad_group_asset",
        "SELECT
          ad_group_asset.resource_name,
          ad_group_asset.ad_group,
          ad_group_asset.asset,
          ad_group_asset.field_type,
          ad_group_asset.source,
          ad_group_asset.status
        FROM ad_group_asset
        WHERE ad_group_asset.status != 'REMOVED'",
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

/// Every field path the live-state fetch names in a `SELECT`. This is the exact
/// set `plan` can compare, so it is also the set an `unchanged` row makes a
/// claim about — everything else on the resource is undiffed (issue #111).
pub fn selected_fields() -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    for (_, query) in QUERIES {
        let Some(rest) = query.trim_start().strip_prefix("SELECT") else {
            continue;
        };
        let select_list = rest.split("FROM").next().unwrap_or_default();
        for field in select_list.split(',') {
            let field = field.trim();
            if !field.is_empty() {
                out.insert(field.to_string());
            }
        }
    }
    out
}

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
#[derive(Clone, Copy, PartialEq)]
pub enum CacheMode {
    /// Read cache if fresh, else fetch and write cache. Default for plan/apply.
    ReadWrite,
    /// Always fetch fresh; still write the result to cache. `--refresh-state`.
    RefreshWrite,
    /// Bypass cache entirely (no read, no write).
    Bypass,
}

impl CacheMode {
    /// Only the default mode may answer from cache. `--refresh-state` promises
    /// a live read and must never be satisfiable by a cache hit, however fresh
    /// the entry looks (issue #144).
    fn reads_cache(self) -> bool {
        self == CacheMode::ReadWrite
    }

    fn writes_cache(self) -> bool {
        self != CacheMode::Bypass
    }
}

/// Where a plan's picture of the account came from, and when. A diff is only
/// ever as true as this snapshot: the operations it builds are checked against
/// the *live* account, so a snapshot the account has moved past produces
/// per-resource API errors that look exactly like a genuinely bad `.bid`
/// (issue #144). Every path that loads state reports one of these.
#[derive(Clone)]
pub struct StateProvenance {
    pub source: StateSource,
    pub customer_id: String,
    /// Unix seconds at which the underlying API read happened — not when it was
    /// loaded, so a cache hit keeps the age of the original fetch.
    pub fetched_at: u64,
}

#[derive(Clone, Copy, PartialEq)]
pub enum StateSource {
    /// Read from the API during this run.
    Fresh,
    /// Reused from `.bidsmith/cache/live-state.json`.
    Cached,
    /// Reused from cache with no API call available at all (`--offline`).
    CachedOffline,
}

impl StateProvenance {
    pub fn age_secs(&self) -> u64 {
        cache::now_unix().saturating_sub(self.fetched_at)
    }

    fn qualifier(&self) -> &'static str {
        match self.source {
            StateSource::Fresh => "fresh read",
            StateSource::Cached => "cached — --refresh-state to refetch",
            StateSource::CachedOffline => "cached — offline, no API call",
        }
    }

    /// The one line every command prints once it knows what it is diffing
    /// against. Same shape on the cache-hit and the fresh-fetch path, so two
    /// runs that disagree can be told apart from their output alone.
    pub fn describe(&self) -> String {
        format!(
            "live state for customers/{} read {} ({})",
            self.customer_id,
            cache::format_age_phrase(self.age_secs()),
            self.qualifier(),
        )
    }

    /// How long after this machine's last real mutate on the same customer the
    /// state was read, when that is recent enough to matter.
    pub fn secs_after_local_mutate(&self) -> Option<u64> {
        let at = cache::load_last_mutate(&cache::project_cache_dir(), &self.customer_id)?;
        let gap = self.fetched_at.saturating_sub(at);
        (self.fetched_at >= at && gap <= MUTATE_SETTLING_SECS).then_some(gap)
    }
}

/// How long after a mutate a live read is still worth flagging as possibly
/// pre-mutate. The Google Ads API is not read-your-writes; a search issued
/// seconds after a batch lands can still answer from the old picture.
pub const MUTATE_SETTLING_SECS: u64 = 60;

pub struct FetchOutcome {
    pub state: ExportInput,
    pub provenance: StateProvenance,
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

    if effective_mode.reads_cache() {
        if let Some(hit) = cache::load_live_state(
            &cache_dir,
            &client.customer_id,
            login,
            &api_v,
            &queries_fp,
            cache::live_state_ttl_secs(),
        ) {
            let provenance = StateProvenance {
                source: StateSource::Cached,
                customer_id: client.customer_id.clone(),
                fetched_at: cache::now_unix().saturating_sub(hit.age_secs),
            };
            eprintln!("{label}: {}.", provenance.describe());
            let state = adapt_batches(hit.batches)?;
            return Ok(FetchOutcome { state, provenance });
        }
    }

    eprintln!(
        "{label}: fetching live state from customers/{}...",
        client.customer_id,
    );
    let batches = fetch_raw(client, access_token)?;
    let provenance = StateProvenance {
        source: StateSource::Fresh,
        customer_id: client.customer_id.clone(),
        fetched_at: cache::now_unix(),
    };

    if effective_mode.writes_cache() {
        let _ = cache::save_live_state(
            &cache_dir,
            &client.customer_id,
            login,
            &api_v,
            &queries_fp,
            &batches,
        );
    }

    eprintln!("{label}: {}.", provenance.describe());
    let state = adapt_batches(batches)?;
    Ok(FetchOutcome { state, provenance })
}

/// Called on the way *into* a real mutate, not on the way out: once the request
/// is in flight the cached state is stale whatever happens next, and a lost
/// response or a killed process must not leave a snapshot that still looks
/// fresh (issue #144).
pub fn note_mutate(customer_id: &str) {
    let dir = cache::project_cache_dir();
    cache::invalidate_live_state(&dir);
    cache::record_last_mutate(&dir, customer_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_fields_parses_every_select_list() {
        let fields = selected_fields();
        assert!(fields.contains("campaign.name"));
        assert!(fields.contains("ad_group.target_cpv_micros"));
        assert!(fields.contains("campaign.geo_target_type_setting.positive_geo_target_type"));
        assert!(
            !fields.iter().any(|f| f.contains("WHERE") || f.contains("FROM")),
            "the parse must stop at FROM: {fields:?}",
        );
        assert!(
            !fields.iter().any(|f| f.contains('\n')),
            "field paths are trimmed of the query's indentation",
        );
    }

    /// `drift` reports what a query does not select, so selecting these is what
    /// moves AI Max out of the "set but never compared" list (issue #158).
    #[test]
    fn both_halves_of_ai_max_are_selected() {
        let fields = selected_fields();
        assert!(fields.contains("campaign.ai_max_setting.enable_ai_max"));
        assert!(fields.contains("ad_group.ai_max_ad_group_setting.disable_search_term_matching"));
    }

    /// Both `channel_controls` arms plus the output-only `channel_config` —
    /// the config is what tells a live explicit ALL_CHANNELS from an unset one
    /// (issue #180).
    #[test]
    fn every_channel_controls_field_is_selected() {
        let fields = selected_fields();
        let prefix = "ad_group.demand_gen_ad_group_settings.channel_controls";
        assert!(fields.contains(&format!("{prefix}.channel_config")));
        assert!(fields.contains(&format!("{prefix}.channel_strategy")));
        for (field, _) in crate::schema::DEMAND_GEN_SELECTED_CHANNEL_FIELDS {
            assert!(
                fields.contains(&format!("{prefix}.selected_channels.{field}")),
                "{field} missing from the ad_group query"
            );
        }
    }

    /// A budget Google removed alongside its last campaign used to stay in
    /// live state and shadow the declared one by name, so the campaign could
    /// not be re-created (issue #161). The status is selected as well as
    /// filtered on, so a row that reaches the matcher can still be judged.
    #[test]
    fn removed_budgets_are_filtered_out_and_their_status_is_readable() {
        let (_, budgets) = QUERIES
            .iter()
            .find(|(label, _)| *label == "campaign_budget")
            .expect("a campaign_budget query");
        assert!(budgets.contains("campaign_budget.status != 'REMOVED'"), "{budgets}");
        assert!(selected_fields().contains("campaign_budget.status"));
    }

    #[test]
    fn refresh_state_can_never_be_answered_from_cache() {
        // The first thing to rule out when a plan disagrees with the account:
        // --refresh-state really does refetch (issue #144).
        assert!(!CacheMode::RefreshWrite.reads_cache());
        assert!(!CacheMode::Bypass.reads_cache());
        assert!(CacheMode::ReadWrite.reads_cache());

        assert!(CacheMode::RefreshWrite.writes_cache(), "a refetch reseeds the cache");
        assert!(CacheMode::ReadWrite.writes_cache());
        assert!(!CacheMode::Bypass.writes_cache());
    }

    #[test]
    fn provenance_reads_the_same_on_both_paths() {
        let fresh = StateProvenance {
            source: StateSource::Fresh,
            customer_id: "1234567890".into(),
            fetched_at: cache::now_unix(),
        };
        assert_eq!(
            fresh.describe(),
            "live state for customers/1234567890 read just now (fresh read)",
        );

        let cached = StateProvenance {
            source: StateSource::Cached,
            customer_id: "1234567890".into(),
            fetched_at: cache::now_unix().saturating_sub(70),
        };
        assert_eq!(
            cached.describe(),
            "live state for customers/1234567890 read 1m10s ago \
             (cached — --refresh-state to refetch)",
        );

        let offline = StateProvenance {
            source: StateSource::CachedOffline,
            customer_id: "1234567890".into(),
            fetched_at: cache::now_unix().saturating_sub(5),
        };
        assert!(offline.describe().contains("offline, no API call"));
    }

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
