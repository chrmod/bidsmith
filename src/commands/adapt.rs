use std::collections::BTreeMap;

use serde_json::Value;

use crate::commands::export::{
    ExportInput, JsonAd, JsonAdGroup, JsonAdGroupAd, JsonAdGroupAsset, JsonAdGroupCriterion,
    JsonAssetAutomationSettings, JsonBidSelector, JsonBudget, JsonCallAsset, JsonCalloutAsset,
    JsonCampaign, JsonCampaignAsset,
    JsonCampaignCriterion, JsonCampaignSharedSet, JsonConversionAction, JsonCriterion,
    JsonCustomerAsset, JsonAgeRange, JsonAudience, JsonCustomAudience, JsonCustomAudienceMember,
    JsonCustomParameter, JsonDemandGenVideoResponsiveAd, JsonDevice, JsonFrequencyCap, JsonGender,
    JsonGeoTargetTypeSetting, JsonIncomeRange, JsonKeyword, JsonLanguage, JsonLocation,
    JsonManualCpc, JsonNetworkSettings, JsonParentalStatus, JsonPlacement, JsonProximity,
    JsonResponsiveSearchAd, JsonRsaAsset, JsonSharedSet, JsonSitelinkAsset,
    JsonStructuredSnippetAsset, JsonTargetImpressionShare, JsonTargetRestriction,
    JsonTargetSpend, JsonTargetingSetting, JsonTopic, JsonUserInterest, JsonValueSettings,
    JsonVideoAd, JsonVideoAdInventoryControl, JsonVideoCampaignSettings, JsonVideoResponsiveAd,
    JsonYoutubeChannel, JsonYoutubeVideo, JsonYoutubeVideoAsset,
};

pub fn from_search_response(raw: &str) -> Result<ExportInput, String> {
    let root: Value = serde_json::from_str(raw).map_err(|e| format!("invalid JSON: {e}"))?;

    let mut state = AdapterState::default();
    for row in collect_rows(&root)? {
        state.absorb_row(row);
    }
    state.into_export_input()
}

fn collect_rows(v: &Value) -> Result<Vec<&Value>, String> {
    match v {
        Value::Object(_) => {
            if let Some(results) = v.get("results").and_then(Value::as_array) {
                Ok(results.iter().collect())
            } else {
                Err("top-level object must have a 'results' array".to_string())
            }
        }
        Value::Array(arr) => {
            let mut out = Vec::new();
            for batch in arr {
                if let Some(results) = batch.get("results").and_then(Value::as_array) {
                    out.extend(results.iter());
                } else if batch.is_object() {
                    out.push(batch);
                } else {
                    return Err(
                        "array element must be a batch object with 'results' or a row".to_string(),
                    );
                }
            }
            Ok(out)
        }
        _ => Err("expected JSON object or array".to_string()),
    }
}

struct SharedSetBuilder {
    id: String,
    name: String,
    ty: Option<String>,
    status: Option<String>,
}

#[derive(Default)]
struct AdapterState {
    customer_id: Option<String>,
    currency_code: Option<String>,
    budgets: BTreeMap<String, JsonBudget>,
    campaigns: BTreeMap<String, JsonCampaign>,
    ad_groups: BTreeMap<String, JsonAdGroup>,
    ad_group_ads: BTreeMap<String, JsonAdGroupAd>,
    ad_group_criteria: BTreeMap<String, JsonAdGroupCriterion>,
    campaign_criteria: BTreeMap<String, JsonCampaignCriterion>,
    conversion_actions: BTreeMap<String, JsonConversionAction>,
    call_assets: BTreeMap<String, JsonCallAsset>,
    sitelink_assets: BTreeMap<String, JsonSitelinkAsset>,
    callout_assets: BTreeMap<String, JsonCalloutAsset>,
    structured_snippet_assets: BTreeMap<String, JsonStructuredSnippetAsset>,
    youtube_video_assets: BTreeMap<String, JsonYoutubeVideoAsset>,
    customer_assets: BTreeMap<String, JsonCustomerAsset>,
    campaign_assets: BTreeMap<String, JsonCampaignAsset>,
    ad_group_assets: BTreeMap<String, JsonAdGroupAsset>,
    shared_sets: BTreeMap<String, SharedSetBuilder>,
    custom_audiences: BTreeMap<String, JsonCustomAudience>,
    // value: (real criterion resource-name segment `<setId>~<critId>`, keyword)
    shared_criteria: BTreeMap<String, Vec<(String, JsonKeyword)>>,
    campaign_shared_sets: BTreeMap<String, JsonCampaignSharedSet>,
    // live entity id -> bidsmith:address, read from label associations.
    campaign_addresses: BTreeMap<String, String>,
    ad_group_addresses: BTreeMap<String, String>,
    ad_group_ad_addresses: BTreeMap<String, String>,
    ad_group_criterion_addresses: BTreeMap<String, String>,
    // live entity id -> claimed criterion categories, from bidsmith:owns labels.
    campaign_claims: BTreeMap<String, Vec<String>>,
    ad_group_claims: BTreeMap<String, Vec<String>>,
    // bidsmith:address -> label resource_name, read from the label resource.
    labels: BTreeMap<String, String>,
    // bidsmith:owns category -> label resource_name.
    claim_labels: BTreeMap<String, String>,
}

impl AdapterState {
    fn absorb_row(&mut self, row: &Value) {
        if let Some(v) = row.get("customer") {
            self.merge_customer(v);
        }
        if let Some(v) = row.get("campaignBudget") {
            self.merge_budget(v);
        }
        if let Some(v) = row.get("campaign") {
            self.merge_campaign(v);
        }
        if let Some(v) = row.get("adGroup") {
            self.merge_ad_group(v);
        }
        if let Some(v) = row.get("adGroupAd") {
            self.merge_ad_group_ad(v);
        }
        if let Some(v) = row.get("adGroupCriterion") {
            self.merge_ad_group_criterion(v);
        }
        if let Some(v) = row.get("campaignCriterion") {
            self.merge_campaign_criterion(v);
        }
        if let Some(v) = row.get("conversionAction") {
            self.merge_conversion_action(v);
        }
        if let Some(v) = row.get("asset") {
            self.merge_asset(v);
        }
        if let Some(v) = row.get("customerAsset") {
            self.merge_customer_asset(v);
        }
        if let Some(v) = row.get("campaignAsset") {
            self.merge_campaign_asset(v);
        }
        if let Some(v) = row.get("adGroupAsset") {
            self.merge_ad_group_asset(v);
        }
        if let Some(v) = row.get("customAudience") {
            self.merge_custom_audience(v);
        }
        if let Some(v) = row.get("sharedSet") {
            self.merge_shared_set(v);
        }
        if let Some(v) = row.get("sharedCriterion") {
            self.merge_shared_criterion(v);
        }
        if let Some(v) = row.get("campaignSharedSet") {
            self.merge_campaign_shared_set(v);
        }
        let label = row.get("label");
        if let Some(v) = label {
            self.merge_label_resource(v);
        }
        if let Some(v) = row.get("campaignLabel") {
            self.merge_label(v, "campaign", label, |s| &mut s.campaign_addresses);
            self.merge_claim(v, "campaign", label, |s| &mut s.campaign_claims);
        }
        if let Some(v) = row.get("adGroupLabel") {
            self.merge_label(v, "adGroup", label, |s| &mut s.ad_group_addresses);
            self.merge_claim(v, "adGroup", label, |s| &mut s.ad_group_claims);
        }
        if let Some(v) = row.get("adGroupAdLabel") {
            self.merge_label(v, "adGroupAd", label, |s| &mut s.ad_group_ad_addresses);
        }
        if let Some(v) = row.get("adGroupCriterionLabel") {
            self.merge_label(v, "adGroupCriterion", label, |s| {
                &mut s.ad_group_criterion_addresses
            });
        }
    }

    /// Record a `bidsmith:address=<addr>` or `bidsmith:owns=<category>` label
    /// resource (payload -> its resource_name) so the mutate builder can reuse
    /// an existing label rather than re-create one. Rows from the association
    /// queries select only `label.name` (no resource_name), so they no-op here;
    /// the standalone `label` query carries both.
    fn merge_label_resource(&mut self, label: &Value) {
        let Some(rn) = label.get("resourceName").and_then(Value::as_str) else {
            return;
        };
        if let Some(address) = label_address(Some(label)) {
            self.labels.insert(address, rn.to_string());
        } else if let Some(category) = claim_category(Some(label)) {
            self.claim_labels.insert(category, rn.to_string());
        }
    }

    /// Record a `bidsmith:address=<addr>` label association: read the address
    /// off the joined `label.name`, the live entity id off the association's
    /// entity-reference field, and store the id -> address mapping. Non-bidsmith
    /// labels (no matching prefix) are ignored.
    fn merge_label(
        &mut self,
        assoc: &Value,
        entity_field: &str,
        label: Option<&Value>,
        select: impl FnOnce(&mut Self) -> &mut BTreeMap<String, String>,
    ) {
        let Some(address) = label_address(label) else {
            return;
        };
        let Some(id) = assoc
            .get(entity_field)
            .and_then(Value::as_str)
            .and_then(last_segment)
        else {
            return;
        };
        select(self).insert(id.to_string(), address);
    }

    /// Record a `bidsmith:owns=<category>` label association: the live parent
    /// (campaign / ad group) claims that criterion category as
    /// bidsmith-managed, so orphaned members prune even after the last declared
    /// member is gone.
    fn merge_claim(
        &mut self,
        assoc: &Value,
        entity_field: &str,
        label: Option<&Value>,
        select: impl FnOnce(&mut Self) -> &mut BTreeMap<String, Vec<String>>,
    ) {
        let Some(category) = claim_category(label) else {
            return;
        };
        let Some(id) = assoc
            .get(entity_field)
            .and_then(Value::as_str)
            .and_then(last_segment)
        else {
            return;
        };
        let claims = select(self).entry(id.to_string()).or_default();
        if !claims.contains(&category) {
            claims.push(category);
        }
    }

    fn merge_customer(&mut self, v: &Value) {
        if let Some(id) = extract_id(v) {
            self.customer_id.get_or_insert(id);
        }
        if let Some(code) = v.get("currencyCode").and_then(Value::as_str) {
            self.currency_code = Some(code.to_string());
        }
    }

    fn note_customer(&mut self, resource_name: &str) {
        if self.customer_id.is_some() {
            return;
        }
        if let Some(tail) = resource_name.strip_prefix("customers/") {
            if let Some(idx) = tail.find('/') {
                self.customer_id = Some(tail[..idx].to_string());
            }
        }
    }

    fn merge_budget(&mut self, v: &Value) {
        if let Some(rn) = v.get("resourceName").and_then(Value::as_str) {
            self.note_customer(rn);
        }
        let Some(id) = extract_id(v) else { return };
        let entry = self.budgets.entry(id.clone()).or_insert_with(|| JsonBudget {
            id,
            name: String::new(),
            amount_micros: None,
            total_amount_micros: None,
            period: None,
            ty: None,
            delivery_method: None,
            explicitly_shared: None,
        });
        if let Some(s) = v.get("name").and_then(Value::as_str) {
            entry.name = s.to_string();
        }
        if let Some(n) = parse_i64(v.get("amountMicros")) {
            entry.amount_micros = Some(n);
        }
        if let Some(n) = parse_i64(v.get("totalAmountMicros")) {
            entry.total_amount_micros = Some(n);
        }
        if let Some(s) = v.get("period").and_then(Value::as_str) {
            entry.period = Some(s.to_string());
        }
        if let Some(s) = v.get("type").and_then(Value::as_str) {
            entry.ty = Some(s.to_string());
        }
        if let Some(s) = v.get("deliveryMethod").and_then(Value::as_str) {
            entry.delivery_method = Some(s.to_string());
        }
        if let Some(b) = v.get("explicitlyShared").and_then(Value::as_bool) {
            entry.explicitly_shared = Some(b);
        }
    }

    fn merge_campaign(&mut self, v: &Value) {
        if let Some(rn) = v.get("resourceName").and_then(Value::as_str) {
            self.note_customer(rn);
        }
        let Some(id) = extract_id(v) else { return };
        let entry = self
            .campaigns
            .entry(id.clone())
            .or_insert_with(|| JsonCampaign {
                id,
                name: String::new(),
                status: None,
                advertising_channel_type: String::new(),
                advertising_channel_sub_type: None,
                campaign_budget: String::new(),
                contains_eu_political_advertising: None,
                start_date: None,
                end_date: None,
                final_url_suffix: None,
                custom_parameters: None,
                manual_cpc: None,
                manual_cpm: None,
                manual_cpv: None,
                target_cpm: None,
                target_cpv: None,
                target_impression_share: None,
                target_spend: None,
                network_settings: None,
                geo_target_type_setting: None,
                video_campaign_settings: None,
                asset_automation_settings: None,
                targeting_setting: None,
                frequency_caps: Vec::new(),
                managed_address: None,
            });
        if let Some(s) = v.get("name").and_then(Value::as_str) {
            entry.name = s.to_string();
        }
        if let Some(s) = v.get("status").and_then(Value::as_str) {
            entry.status = Some(s.to_string());
        }
        if let Some(s) = v.get("advertisingChannelType").and_then(Value::as_str) {
            entry.advertising_channel_type = s.to_string();
        }
        // A campaign that refines its channel no further reports `UNSPECIFIED`,
        // which is a report rather than a setting — carrying it over would
        // render a `.bid` the validator rejects.
        if let Some(s) = v.get("advertisingChannelSubType").and_then(Value::as_str) {
            entry.advertising_channel_sub_type = Some(s.to_string())
                .filter(|s| crate::schema::is_advertising_channel_sub_type(s));
        }
        if let Some(s) = v.get("startDate").and_then(Value::as_str) {
            entry.start_date = Some(s.to_string());
        }
        // Google writes a far-future sentinel rather than clearing the field, so
        // an open-ended campaign reads as unset instead of baking a fake date
        // into every adopted `.bid` (issue #113).
        if let Some(s) = v.get("endDate").and_then(Value::as_str) {
            entry.end_date = Some(s.to_string()).filter(|d| d != crate::schema::NO_END_DATE);
        }
        if let Some(rn) = v.get("campaignBudget").and_then(Value::as_str) {
            if let Some(id) = last_segment(rn) {
                entry.campaign_budget = id.to_string();
            }
        }
        if let Some(s) = v
            .get("containsEuPoliticalAdvertising")
            .and_then(Value::as_str)
        {
            entry.contains_eu_political_advertising = Some(s.to_string());
        }
        if let Some(mc) = v.get("manualCpc") {
            entry.manual_cpc = Some(JsonManualCpc {
                enhanced_cpc_enabled: mc
                    .get("enhancedCpcEnabled")
                    .and_then(Value::as_bool),
            });
        } else if let Some(tis) = v.get("targetImpressionShare") {
            entry.target_impression_share = Some(JsonTargetImpressionShare {
                location: tis.get("location").and_then(Value::as_str).map(str::to_string),
                location_fraction_micros: parse_i64(tis.get("locationFractionMicros")),
                cpc_bid_ceiling_micros: parse_i64(tis.get("cpcBidCeilingMicros")),
            });
        } else if let Some(ts) = v.get("targetSpend") {
            entry.target_spend = Some(JsonTargetSpend {
                cpc_bid_ceiling_micros: parse_i64(ts.get("cpcBidCeilingMicros")),
            });
        } else {
            // The video strategies are empty messages, so GAQL exposes no leaf
            // field to select — `bidding_strategy_type` is the only tell. The
            // last two have leaves, but a live strategy whose every field is
            // unset comes back with no message object at all.
            match v.get("biddingStrategyType").and_then(Value::as_str) {
                Some("MANUAL_CPM") => entry.manual_cpm = Some(JsonBidSelector {}),
                Some("MANUAL_CPV") => entry.manual_cpv = Some(JsonBidSelector {}),
                Some("TARGET_CPM") => entry.target_cpm = Some(JsonBidSelector {}),
                Some("TARGET_CPV") => entry.target_cpv = Some(JsonBidSelector {}),
                Some("TARGET_IMPRESSION_SHARE") => {
                    entry.target_impression_share = Some(JsonTargetImpressionShare::default())
                }
                Some("TARGET_SPEND") => entry.target_spend = Some(JsonTargetSpend::default()),
                _ => {}
            }
        }
        if let Some(caps) = v.get("frequencyCaps").and_then(Value::as_array) {
            entry.frequency_caps = caps.iter().filter_map(parse_frequency_cap).collect();
        }
        if let Some(ns) = v.get("networkSettings") {
            let mut settings = JsonNetworkSettings::default();
            for (field, json) in crate::schema::NETWORK_SETTINGS_FIELDS {
                settings.set(field, ns.get(json).and_then(Value::as_bool));
            }
            entry.network_settings = Some(settings);
        }
        // Google reports a geo target type it has no value for as `UNKNOWN`,
        // which is not a setting anyone can declare — carrying it over would
        // render a `.bid` the validator rejects and drift that never resolves.
        if let Some(gs) = v.get("geoTargetTypeSetting") {
            let mut setting = JsonGeoTargetTypeSetting::default();
            for (field, json) in crate::schema::GEO_TARGET_TYPE_FIELDS {
                let live = gs
                    .get(json)
                    .and_then(Value::as_str)
                    .filter(|s| crate::schema::is_geo_target_type(s))
                    .map(str::to_string);
                setting.set(field, live);
            }
            if !setting.is_empty() {
                entry.geo_target_type_setting = Some(setting);
            }
        }
        if let Some(ic) = v
            .get("videoCampaignSettings")
            .and_then(|s| s.get("videoAdInventoryControl"))
        {
            let mut control = JsonVideoAdInventoryControl::default();
            for (field, json) in crate::schema::VIDEO_AD_INVENTORY_FIELDS {
                control.set(field, ic.get(json).and_then(Value::as_bool));
            }
            entry.video_campaign_settings = Some(JsonVideoCampaignSettings {
                video_ad_inventory_control: Some(control),
            });
        }
        // An automation this build has no attribute for, or a status it has no
        // name for, is a report rather than a setting: carrying either over
        // would render a `.bid` the validator rejects.
        if let Some(list) = v.get("assetAutomationSettings").and_then(Value::as_array) {
            let mut settings = JsonAssetAutomationSettings::default();
            for setting in list {
                let field = setting
                    .get("assetAutomationType")
                    .and_then(Value::as_str)
                    .and_then(crate::schema::asset_automation_field);
                let status = setting
                    .get("assetAutomationStatus")
                    .and_then(Value::as_str)
                    .filter(|s| crate::schema::is_asset_automation_status(s));
                if let (Some(field), Some(status)) = (field, status) {
                    settings.set(field, Some(status.to_string()));
                }
            }
            if !settings.is_empty() {
                entry.asset_automation_settings = Some(settings);
            }
        }
        if v.get("targetingSetting").is_some() {
            entry.targeting_setting = parse_targeting_setting(v);
        }
        if let Some(s) = v.get("finalUrlSuffix").and_then(Value::as_str) {
            entry.final_url_suffix = Some(s.to_string());
        }
        if let Some(params) = parse_custom_parameters(v) {
            entry.custom_parameters = Some(params);
        }
    }

    fn merge_ad_group(&mut self, v: &Value) {
        if let Some(rn) = v.get("resourceName").and_then(Value::as_str) {
            self.note_customer(rn);
        }
        let Some(id) = extract_id(v) else { return };
        let entry = self.ad_groups.entry(id.clone()).or_insert_with(|| JsonAdGroup {
            id,
            ..Default::default()
        });
        if let Some(s) = v.get("name").and_then(Value::as_str) {
            entry.name = s.to_string();
        }
        if let Some(rn) = v.get("campaign").and_then(Value::as_str) {
            if let Some(id) = last_segment(rn) {
                entry.campaign = id.to_string();
            }
        }
        if let Some(s) = v.get("status").and_then(Value::as_str) {
            entry.status = Some(s.to_string());
        }
        if let Some(s) = v.get("type").and_then(Value::as_str) {
            entry.ty = Some(s.to_string());
        }
        if let Some(s) = v.get("finalUrlSuffix").and_then(Value::as_str) {
            entry.final_url_suffix = Some(s.to_string());
        }
        if let Some(params) = parse_custom_parameters(v) {
            entry.custom_parameters = Some(params);
        }
        for (field, json) in crate::schema::AD_GROUP_BID_FIELDS {
            if let Some(n) = parse_i64(v.get(json)) {
                entry.set_bid(field, Some(n));
            }
        }
        // Only when the row carries the field: a row from another query that
        // merely mentions this ad group must not blank out what one that
        // selected it already read.
        if v.get("targetingSetting").is_some() {
            entry.targeting_setting = parse_targeting_setting(v);
        }
    }

    fn merge_ad_group_ad(&mut self, v: &Value) {
        if let Some(rn) = v.get("resourceName").and_then(Value::as_str) {
            self.note_customer(rn);
        }
        let Some(key) = v
            .get("resourceName")
            .and_then(Value::as_str)
            .and_then(last_segment)
            .map(str::to_string)
            .or_else(|| extract_id(v))
        else {
            return;
        };
        let ad_group_id = v
            .get("adGroup")
            .and_then(Value::as_str)
            .and_then(last_segment)
            .unwrap_or("")
            .to_string();
        let entry = self
            .ad_group_ads
            .entry(key.clone())
            .or_insert_with(|| JsonAdGroupAd {
                id: key,
                ad_group: String::new(),
                status: None,
                ad: JsonAd {
                    name: None,
                    final_urls: Vec::new(),
                    final_mobile_urls: Vec::new(),
                    display_url: None,
                    final_url_suffix: None,
                    custom_parameters: None,
                    responsive_search_ad: None,
                    video_responsive_ad: None,
                    video_ad: None,
                    demand_gen_video_responsive_ad: None,
                },
                managed_address: None,
            });
        if !ad_group_id.is_empty() {
            entry.ad_group = ad_group_id;
        }
        if let Some(s) = v.get("status").and_then(Value::as_str) {
            entry.status = Some(s.to_string());
        }
        if let Some(ad) = v.get("ad") {
            if let Some(s) = ad.get("name").and_then(Value::as_str) {
                entry.ad.name = Some(s.to_string());
            }
            if let Some(s) = ad.get("finalUrlSuffix").and_then(Value::as_str) {
                entry.ad.final_url_suffix = Some(s.to_string());
            }
            if let Some(params) = parse_custom_parameters(ad) {
                entry.ad.custom_parameters = Some(params);
            }
            if let Some(urls) = ad.get("finalUrls").and_then(Value::as_array) {
                let urls: Vec<String> = urls
                    .iter()
                    .filter_map(|u| u.as_str().map(String::from))
                    .collect();
                if !urls.is_empty() {
                    entry.ad.final_urls = urls;
                }
            }
            if let Some(urls) = ad.get("finalMobileUrls").and_then(Value::as_array) {
                let urls: Vec<String> = urls
                    .iter()
                    .filter_map(|u| u.as_str().map(String::from))
                    .collect();
                if !urls.is_empty() {
                    entry.ad.final_mobile_urls = urls;
                }
            }
            if let Some(u) = ad.get("displayUrl").and_then(Value::as_str) {
                entry.ad.display_url = Some(u.to_string());
            }
            if let Some(rsa) = ad.get("responsiveSearchAd") {
                entry.ad.responsive_search_ad = Some(JsonResponsiveSearchAd {
                    headlines: extract_rsa_assets(rsa.get("headlines")),
                    descriptions: extract_rsa_assets(rsa.get("descriptions")),
                    path1: rsa.get("path1").and_then(Value::as_str).map(String::from),
                    path2: rsa.get("path2").and_then(Value::as_str).map(String::from),
                });
            }
            if let Some(video) = ad.get("videoResponsiveAd") {
                entry.ad.video_responsive_ad = Some(JsonVideoResponsiveAd {
                    video: extract_video_asset_ids(video.get("videos"))
                        .into_iter()
                        .next()
                        .unwrap_or_default(),
                    headlines: extract_ad_text_list(video.get("headlines")),
                    long_headlines: extract_ad_text_list(video.get("longHeadlines")),
                    descriptions: extract_ad_text_list(video.get("descriptions")),
                    call_to_actions: extract_ad_text_list(video.get("callToActions")),
                    breadcrumb1: video.get("breadcrumb1").and_then(Value::as_str).map(String::from),
                    breadcrumb2: video.get("breadcrumb2").and_then(Value::as_str).map(String::from),
                });
            }
            if let Some(video) = ad
                .get("videoAd")
                .and_then(|v| v.get("video"))
                .and_then(|v| v.get("asset"))
                .and_then(Value::as_str)
                .and_then(last_segment)
            {
                entry.ad.video_ad = Some(JsonVideoAd {
                    video: video.to_string(),
                });
            }
            if let Some(dg) = ad.get("demandGenVideoResponsiveAd") {
                entry.ad.demand_gen_video_responsive_ad = Some(JsonDemandGenVideoResponsiveAd {
                    videos: extract_video_asset_ids(dg.get("videos")),
                    headlines: extract_ad_text_list(dg.get("headlines")),
                    long_headlines: extract_ad_text_list(dg.get("longHeadlines")),
                    descriptions: extract_ad_text_list(dg.get("descriptions")),
                    call_to_actions: extract_ad_text_list(dg.get("callToActions")),
                    breadcrumb1: dg.get("breadcrumb1").and_then(Value::as_str).map(String::from),
                    breadcrumb2: dg.get("breadcrumb2").and_then(Value::as_str).map(String::from),
                    business_name: dg
                        .get("businessName")
                        .and_then(|b| b.as_str().map(String::from).or_else(|| {
                            b.get("text").and_then(Value::as_str).map(String::from)
                        })),
                });
            }
        }
    }

    fn merge_ad_group_criterion(&mut self, v: &Value) {
        if let Some(rn) = v.get("resourceName").and_then(Value::as_str) {
            self.note_customer(rn);
        }
        let Some(key) = v
            .get("resourceName")
            .and_then(Value::as_str)
            .and_then(last_segment)
            .map(str::to_string)
        else {
            return;
        };
        let mut target = JsonCriterion::default();
        merge_criterion(v, &mut target);
        // A criterion of a type bidsmith does not model has nothing to render;
        // dropping it here keeps it out of the diff rather than exporting an
        // empty resource.
        if target.is_unset() {
            return;
        }
        let ad_group_id = v
            .get("adGroup")
            .and_then(Value::as_str)
            .and_then(last_segment)
            .unwrap_or("")
            .to_string();
        let entry = self
            .ad_group_criteria
            .entry(key.clone())
            .or_insert_with(|| JsonAdGroupCriterion {
                id: key,
                ad_group: String::new(),
                status: None,
                negative: None,
                cpc_bid_micros: None,
                bid_modifier: None,
                target: JsonCriterion::default(),
                managed_address: None,
            });
        if !ad_group_id.is_empty() {
            entry.ad_group = ad_group_id;
        }
        entry.target = target;
        if let Some(s) = v.get("status").and_then(Value::as_str) {
            entry.status = Some(s.to_string());
        }
        if let Some(b) = v.get("negative").and_then(Value::as_bool) {
            entry.negative = Some(b);
        }
        if let Some(n) = parse_i64(v.get("cpcBidMicros")) {
            entry.cpc_bid_micros = Some(n);
        }
        if let Some(bm) = v.get("bidModifier").and_then(parse_f64_value) {
            entry.bid_modifier = Some(bm);
        }
    }

    fn merge_campaign_criterion(&mut self, v: &Value) {
        if let Some(rn) = v.get("resourceName").and_then(Value::as_str) {
            self.note_customer(rn);
        }
        let Some(key) = v
            .get("resourceName")
            .and_then(Value::as_str)
            .and_then(last_segment)
            .map(str::to_string)
        else {
            return;
        };
        let campaign_id = v
            .get("campaign")
            .and_then(Value::as_str)
            .and_then(last_segment)
            .unwrap_or("")
            .to_string();
        let entry = self
            .campaign_criteria
            .entry(key.clone())
            .or_insert_with(|| JsonCampaignCriterion {
                id: key,
                campaign: String::new(),
                status: None,
                negative: None,
                bid_modifier: None,
                target: JsonCriterion::default(),
            });
        if !campaign_id.is_empty() {
            entry.campaign = campaign_id;
        }
        if let Some(s) = v.get("status").and_then(Value::as_str) {
            entry.status = Some(s.to_string());
        }
        if let Some(b) = v.get("negative").and_then(Value::as_bool) {
            entry.negative = Some(b);
        }
        if let Some(bm) = v.get("bidModifier").and_then(parse_f64_value) {
            entry.bid_modifier = Some(bm);
        }
        merge_criterion(v, &mut entry.target);
    }

    fn merge_custom_audience(&mut self, v: &Value) {
        if let Some(rn) = v.get("resourceName").and_then(Value::as_str) {
            self.note_customer(rn);
        }
        let Some(id) = extract_id(v) else { return };
        let entry = self
            .custom_audiences
            .entry(id.clone())
            .or_insert_with(|| JsonCustomAudience {
                id,
                name: String::new(),
                description: None,
                ty: None,
                status: None,
                members: Vec::new(),
            });
        if let Some(s) = v.get("name").and_then(Value::as_str) {
            entry.name = s.to_string();
        }
        if let Some(s) = v.get("description").and_then(Value::as_str) {
            entry.description = Some(s.to_string());
        }
        if let Some(s) = v.get("type").and_then(Value::as_str) {
            entry.ty = Some(s.to_string());
        }
        if let Some(s) = v.get("status").and_then(Value::as_str) {
            entry.status = Some(s.to_string());
        }
        if let Some(members) = v.get("members").and_then(Value::as_array) {
            entry.members = members.iter().filter_map(parse_custom_audience_member).collect();
        }
    }

    fn merge_conversion_action(&mut self, v: &Value) {
        if let Some(rn) = v.get("resourceName").and_then(Value::as_str) {
            self.note_customer(rn);
        }
        let Some(id) = extract_id(v) else { return };
        let entry = self
            .conversion_actions
            .entry(id.clone())
            .or_insert_with(|| JsonConversionAction {
                id,
                name: String::new(),
                ty: String::new(),
                category: String::new(),
                status: None,
                counting_type: None,
                click_through_lookback_window_days: None,
                view_through_lookback_window_days: None,
                value_settings: None,
            });
        if let Some(s) = v.get("name").and_then(Value::as_str) {
            entry.name = s.to_string();
        }
        if let Some(s) = v.get("type").and_then(Value::as_str) {
            entry.ty = s.to_string();
        }
        if let Some(s) = v.get("category").and_then(Value::as_str) {
            entry.category = s.to_string();
        }
        if let Some(s) = v.get("status").and_then(Value::as_str) {
            entry.status = Some(s.to_string());
        }
        if let Some(s) = v.get("countingType").and_then(Value::as_str) {
            entry.counting_type = Some(s.to_string());
        }
        if let Some(n) = parse_i64(v.get("clickThroughLookbackWindowDays")) {
            entry.click_through_lookback_window_days = Some(n);
        }
        if let Some(n) = parse_i64(v.get("viewThroughLookbackWindowDays")) {
            entry.view_through_lookback_window_days = Some(n);
        }
        if let Some(vs) = v.get("valueSettings") {
            entry.value_settings = Some(JsonValueSettings {
                default_value: vs.get("defaultValue").and_then(parse_f64_value),
                default_currency_code: vs
                    .get("defaultCurrencyCode")
                    .and_then(Value::as_str)
                    .map(String::from),
                always_use_default_value: vs
                    .get("alwaysUseDefaultValue")
                    .and_then(Value::as_bool),
            });
        }
    }

    fn merge_asset(&mut self, v: &Value) {
        if let Some(rn) = v.get("resourceName").and_then(Value::as_str) {
            self.note_customer(rn);
        }
        let Some(id) = extract_id(v) else { return };
        if let Some(call) = v.get("callAsset") {
            let entry = self
                .call_assets
                .entry(id.clone())
                .or_insert_with(|| JsonCallAsset {
                    id: id.clone(),
                    country_code: String::new(),
                    phone_number: String::new(),
                    call_conversion_reporting_state: None,
                    call_conversion_action: None,
                });
            if let Some(s) = call.get("countryCode").and_then(Value::as_str) {
                entry.country_code = s.to_string();
            }
            if let Some(s) = call.get("phoneNumber").and_then(Value::as_str) {
                entry.phone_number = s.to_string();
            }
            if let Some(s) = call.get("callConversionReportingState").and_then(Value::as_str) {
                entry.call_conversion_reporting_state = Some(s.to_string());
            }
            if let Some(rn) = call.get("callConversionAction").and_then(Value::as_str) {
                entry.call_conversion_action = Some(rn.to_string());
            }
        }
        if let Some(sl) = v.get("sitelinkAsset") {
            let entry = self
                .sitelink_assets
                .entry(id.clone())
                .or_insert_with(|| JsonSitelinkAsset {
                    id: id.clone(),
                    link_text: String::new(),
                    description1: None,
                    description2: None,
                    final_urls: Vec::new(),
                });
            if let Some(s) = sl.get("linkText").and_then(Value::as_str) {
                entry.link_text = s.to_string();
            }
            if let Some(s) = sl.get("description1").and_then(Value::as_str) {
                entry.description1 = Some(s.to_string());
            }
            if let Some(s) = sl.get("description2").and_then(Value::as_str) {
                entry.description2 = Some(s.to_string());
            }
            if let Some(urls) = v.get("finalUrls").and_then(Value::as_array) {
                entry.final_urls = urls
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect();
            }
        }
        if let Some(co) = v.get("calloutAsset") {
            let entry = self
                .callout_assets
                .entry(id.clone())
                .or_insert_with(|| JsonCalloutAsset {
                    id: id.clone(),
                    text: String::new(),
                });
            if let Some(s) = co.get("calloutText").and_then(Value::as_str) {
                entry.text = s.to_string();
            }
        }
        if let Some(ss) = v.get("structuredSnippetAsset") {
            let entry = self
                .structured_snippet_assets
                .entry(id.clone())
                .or_insert_with(|| JsonStructuredSnippetAsset {
                    id: id.clone(),
                    header: String::new(),
                    values: Vec::new(),
                });
            if let Some(s) = ss.get("header").and_then(Value::as_str) {
                entry.header = s.to_string();
            }
            if let Some(vals) = ss.get("values").and_then(Value::as_array) {
                entry.values = vals
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect();
            }
        }
        if let Some(yt) = v.get("youtubeVideoAsset") {
            let entry = self
                .youtube_video_assets
                .entry(id.clone())
                .or_insert_with(|| JsonYoutubeVideoAsset {
                    id: id.clone(),
                    youtube_video_id: String::new(),
                    youtube_video_title: None,
                });
            if let Some(s) = yt.get("youtubeVideoId").and_then(Value::as_str) {
                entry.youtube_video_id = s.to_string();
            }
            if let Some(s) = yt.get("youtubeVideoTitle").and_then(Value::as_str) {
                entry.youtube_video_title = Some(s.to_string());
            }
        }
    }

    fn merge_customer_asset(&mut self, v: &Value) {
        if let Some(rn) = v.get("resourceName").and_then(Value::as_str) {
            self.note_customer(rn);
        }
        let Some(key) = v
            .get("resourceName")
            .and_then(Value::as_str)
            .and_then(last_segment)
            .map(str::to_string)
            .or_else(|| extract_id(v))
        else {
            return;
        };
        let entry = self
            .customer_assets
            .entry(key.clone())
            .or_insert_with(|| JsonCustomerAsset {
                id: key,
                asset: String::new(),
                field_type: String::new(),
                source: None,
                status: None,
            });
        if let Some(rn) = v.get("asset").and_then(Value::as_str) {
            if let Some(id) = last_segment(rn) {
                entry.asset = id.to_string();
            }
        }
        if let Some(s) = v.get("fieldType").and_then(Value::as_str) {
            entry.field_type = s.to_string();
        }
        if let Some(s) = v.get("source").and_then(Value::as_str) {
            entry.source = Some(s.to_string());
        }
        if let Some(s) = v.get("status").and_then(Value::as_str) {
            entry.status = Some(s.to_string());
        }
    }

    fn merge_campaign_asset(&mut self, v: &Value) {
        if let Some(rn) = v.get("resourceName").and_then(Value::as_str) {
            self.note_customer(rn);
        }
        let Some(key) = v
            .get("resourceName")
            .and_then(Value::as_str)
            .and_then(last_segment)
            .map(str::to_string)
        else {
            return;
        };
        let entry = self
            .campaign_assets
            .entry(key.clone())
            .or_insert_with(|| JsonCampaignAsset {
                id: key,
                campaign: String::new(),
                asset: String::new(),
                field_type: String::new(),
                source: None,
                status: None,
            });
        if let Some(rn) = v.get("campaign").and_then(Value::as_str) {
            if let Some(id) = last_segment(rn) {
                entry.campaign = id.to_string();
            }
        }
        if let Some(rn) = v.get("asset").and_then(Value::as_str) {
            if let Some(id) = last_segment(rn) {
                entry.asset = id.to_string();
            }
        }
        if let Some(s) = v.get("fieldType").and_then(Value::as_str) {
            entry.field_type = s.to_string();
        }
        if let Some(s) = v.get("source").and_then(Value::as_str) {
            entry.source = Some(s.to_string());
        }
        if let Some(s) = v.get("status").and_then(Value::as_str) {
            entry.status = Some(s.to_string());
        }
    }

    fn merge_ad_group_asset(&mut self, v: &Value) {
        if let Some(rn) = v.get("resourceName").and_then(Value::as_str) {
            self.note_customer(rn);
        }
        let Some(key) = v
            .get("resourceName")
            .and_then(Value::as_str)
            .and_then(last_segment)
            .map(str::to_string)
        else {
            return;
        };
        let entry = self
            .ad_group_assets
            .entry(key.clone())
            .or_insert_with(|| JsonAdGroupAsset {
                id: key,
                ad_group: String::new(),
                asset: String::new(),
                field_type: String::new(),
                source: None,
                status: None,
            });
        if let Some(rn) = v.get("adGroup").and_then(Value::as_str) {
            if let Some(id) = last_segment(rn) {
                entry.ad_group = id.to_string();
            }
        }
        if let Some(rn) = v.get("asset").and_then(Value::as_str) {
            if let Some(id) = last_segment(rn) {
                entry.asset = id.to_string();
            }
        }
        if let Some(s) = v.get("fieldType").and_then(Value::as_str) {
            entry.field_type = s.to_string();
        }
        if let Some(s) = v.get("source").and_then(Value::as_str) {
            entry.source = Some(s.to_string());
        }
        if let Some(s) = v.get("status").and_then(Value::as_str) {
            entry.status = Some(s.to_string());
        }
    }

    fn merge_shared_set(&mut self, v: &Value) {
        if let Some(rn) = v.get("resourceName").and_then(Value::as_str) {
            self.note_customer(rn);
        }
        let Some(id) = extract_id(v) else { return };
        let entry = self
            .shared_sets
            .entry(id.clone())
            .or_insert_with(|| SharedSetBuilder {
                id: id.clone(),
                name: String::new(),
                ty: None,
                status: None,
            });
        if let Some(s) = v.get("name").and_then(Value::as_str) {
            entry.name = s.to_string();
        }
        if let Some(s) = v.get("type").and_then(Value::as_str) {
            entry.ty = Some(s.to_string());
        }
        if let Some(s) = v.get("status").and_then(Value::as_str) {
            entry.status = Some(s.to_string());
        }
    }

    fn merge_shared_criterion(&mut self, v: &Value) {
        let Some(set_id) = v
            .get("sharedSet")
            .and_then(Value::as_str)
            .and_then(last_segment)
            .map(str::to_string)
        else {
            return;
        };
        let criterion_id = v
            .get("resourceName")
            .and_then(Value::as_str)
            .and_then(last_segment)
            .unwrap_or("")
            .to_string();
        let Some(kw) = v.get("keyword") else { return };
        let text = kw.get("text").and_then(Value::as_str).unwrap_or("").to_string();
        let match_type = kw
            .get("matchType")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if text.is_empty() || match_type.is_empty() {
            return;
        }
        self.shared_criteria
            .entry(set_id)
            .or_default()
            .push((criterion_id, JsonKeyword { text, match_type }));
    }

    fn merge_campaign_shared_set(&mut self, v: &Value) {
        if let Some(rn) = v.get("resourceName").and_then(Value::as_str) {
            self.note_customer(rn);
        }
        let Some(key) = v
            .get("resourceName")
            .and_then(Value::as_str)
            .and_then(last_segment)
            .map(str::to_string)
        else {
            return;
        };
        let campaign_id = v
            .get("campaign")
            .and_then(Value::as_str)
            .and_then(last_segment)
            .unwrap_or("")
            .to_string();
        let shared_set_id = v
            .get("sharedSet")
            .and_then(Value::as_str)
            .and_then(last_segment)
            .unwrap_or("")
            .to_string();
        let entry = self
            .campaign_shared_sets
            .entry(key.clone())
            .or_insert_with(|| JsonCampaignSharedSet {
                id: key,
                campaign: String::new(),
                shared_set: String::new(),
                status: None,
            });
        if !campaign_id.is_empty() {
            entry.campaign = campaign_id;
        }
        if !shared_set_id.is_empty() {
            entry.shared_set = shared_set_id;
        }
        if let Some(s) = v.get("status").and_then(Value::as_str) {
            entry.status = Some(s.to_string());
        }
    }

    fn into_export_input(mut self) -> Result<ExportInput, String> {
        let customer_id = self
            .customer_id
            .ok_or_else(|| "could not determine customer_id from any resourceName".to_string())?;

        for (id, addr) in std::mem::take(&mut self.campaign_addresses) {
            if let Some(c) = self.campaigns.get_mut(&id) {
                c.managed_address = Some(addr);
            }
        }
        for (id, addr) in std::mem::take(&mut self.ad_group_addresses) {
            if let Some(g) = self.ad_groups.get_mut(&id) {
                g.managed_address = Some(addr);
            }
        }
        for (id, addr) in std::mem::take(&mut self.ad_group_ad_addresses) {
            if let Some(a) = self.ad_group_ads.get_mut(&id) {
                a.managed_address = Some(addr);
            }
        }
        for (id, addr) in std::mem::take(&mut self.ad_group_criterion_addresses) {
            if let Some(c) = self.ad_group_criteria.get_mut(&id) {
                c.managed_address = Some(addr);
            }
        }

        let mut shared_criteria_out: Vec<crate::commands::export::JsonSharedCriterion> = Vec::new();
        let shared_sets: Vec<JsonSharedSet> = self
            .shared_sets
            .into_values()
            .map(|s| {
                let SharedSetBuilder {
                    id,
                    name,
                    ty,
                    status,
                } = s;
                let members = self.shared_criteria.remove(&id).unwrap_or_default();
                let mut keywords: Vec<JsonKeyword> = Vec::with_capacity(members.len());
                for (i, (criterion_id, kw)) in members.into_iter().enumerate() {
                    let member_id = if criterion_id.is_empty() {
                        format!("{id}~{i}")
                    } else {
                        criterion_id
                    };
                    shared_criteria_out.push(crate::commands::export::JsonSharedCriterion {
                        id: member_id,
                        shared_set: id.clone(),
                        keyword: kw.clone(),
                    });
                    keywords.push(kw);
                }
                JsonSharedSet {
                    id,
                    name,
                    ty,
                    status,
                    negative_keywords: keywords,
                }
            })
            .collect();
        Ok(ExportInput {
            customer_id,
            login_customer_id: None,
            currency_code: self.currency_code,
            campaign_budgets: self.budgets.into_values().collect(),
            campaigns: self.campaigns.into_values().collect(),
            ad_groups: self.ad_groups.into_values().collect(),
            ad_group_ads: self.ad_group_ads.into_values().collect(),
            ad_group_criteria: self.ad_group_criteria.into_values().collect(),
            campaign_criteria: self.campaign_criteria.into_values().collect(),
            conversion_actions: self.conversion_actions.into_values().collect(),
            call_assets: self.call_assets.into_values().collect(),
            sitelink_assets: self.sitelink_assets.into_values().collect(),
            callout_assets: self.callout_assets.into_values().collect(),
            structured_snippet_assets: self.structured_snippet_assets.into_values().collect(),
            customer_assets: self.customer_assets.into_values().collect(),
            campaign_assets: self.campaign_assets.into_values().collect(),
            ad_group_assets: self.ad_group_assets.into_values().collect(),
            shared_sets,
            shared_criteria: shared_criteria_out,
            campaign_shared_sets: self.campaign_shared_sets.into_values().collect(),
            youtube_video_assets: self.youtube_video_assets.into_values().collect(),
            custom_audiences: self.custom_audiences.into_values().collect(),
            adopt_only: Default::default(),
            owned_account_assets: Default::default(),
            labels: self.labels.into_iter().collect(),
            claim_labels: self.claim_labels.into_iter().collect(),
            campaign_claims: self.campaign_claims.into_iter().collect(),
            ad_group_claims: self.ad_group_claims.into_iter().collect(),
        })
    }
}

fn extract_id(v: &Value) -> Option<String> {
    if let Some(id) = v.get("id") {
        if let Some(s) = id.as_str() {
            return Some(s.to_string());
        }
        if let Some(n) = id.as_i64() {
            return Some(n.to_string());
        }
    }
    v.get("resourceName")
        .and_then(Value::as_str)
        .and_then(last_segment)
        .map(str::to_string)
}

fn last_segment(s: &str) -> Option<&str> {
    s.rsplit('/').next()
}

fn label_address(label: Option<&Value>) -> Option<String> {
    let name = label?.get("name").and_then(Value::as_str)?;
    name.strip_prefix(crate::commands::export::ADDRESS_LABEL_PREFIX)
        .map(str::to_string)
}

fn claim_category(label: Option<&Value>) -> Option<String> {
    let name = label?.get("name").and_then(Value::as_str)?;
    name.strip_prefix(crate::commands::export::OWNS_LABEL_PREFIX)
        .map(str::to_string)
}

fn parse_frequency_cap(v: &Value) -> Option<JsonFrequencyCap> {
    let key = v.get("key")?;
    Some(JsonFrequencyCap {
        event_type: key.get("eventType").and_then(Value::as_str)?.to_string(),
        time_unit: key.get("timeUnit").and_then(Value::as_str)?.to_string(),
        time_length: parse_i64(key.get("timeLength")).unwrap_or(1),
        cap: parse_i64(v.get("cap"))?,
        level: key
            .get("level")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

/// A live targeting setting, or `None` when it says only what the API would
/// assume anyway. Google fills in a restriction for every dimension it has an
/// opinion about, and reading those back as a declaration would put a dozen
/// lines of boilerplate in every ad group (issue #135). Dimensions no `.bid` can
/// declare are dropped, like the geo `UNKNOWN` sentinel above.
/// `urlCustomParameters` as a sorted list, matching the order the importer
/// builds from a map so the two sides of a diff line up.
fn parse_custom_parameters(v: &Value) -> Option<Vec<JsonCustomParameter>> {
    let arr = v.get("urlCustomParameters")?.as_array()?;
    let mut out: Vec<JsonCustomParameter> = arr
        .iter()
        .filter_map(|p| {
            Some(JsonCustomParameter {
                key: p.get("key").and_then(Value::as_str)?.to_string(),
                value: p.get("value").and_then(Value::as_str).unwrap_or("").to_string(),
            })
        })
        .collect();
    out.sort_by(|a, b| a.key.cmp(&b.key));
    Some(out)
}

fn parse_targeting_setting(v: &Value) -> Option<JsonTargetingSetting> {
    let restrictions = v
        .get("targetingSetting")?
        .get("targetRestrictions")?
        .as_array()?;
    let setting = JsonTargetingSetting {
        target_restrictions: restrictions
            .iter()
            .filter_map(|r| {
                Some(JsonTargetRestriction {
                    targeting_dimension: r
                        .get("targetingDimension")
                        .and_then(Value::as_str)
                        .filter(|d| crate::schema::is_targeting_dimension(d))?
                        .to_string(),
                    bid_only: r.get("bidOnly").and_then(Value::as_bool)?,
                })
            })
            .collect(),
    };
    (!setting.effective().is_empty()).then_some(setting)
}

/// Read whichever criterion `oneof` a live row carries. One reader for both
/// criterion resources: the sub-messages are the same on the wire, only the set
/// each resource may carry differs.
fn merge_criterion(v: &Value, target: &mut JsonCriterion) {
    if let Some(kw) = v.get("keyword") {
        target.keyword = Some(JsonKeyword {
            text: kw.get("text").and_then(Value::as_str).unwrap_or("").to_string(),
            match_type: kw
                .get("matchType")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        });
    }
    if let Some(s) = nested_str(v, "location", "geoTargetConstant") {
        target.location = Some(JsonLocation {
            geo_target_constant: s,
        });
    }
    if let Some(s) = nested_str(v, "language", "languageConstant") {
        target.language = Some(JsonLanguage {
            language_constant: s,
        });
    }
    if let Some(prox) = v.get("proximity") {
        let geo = prox.get("geoPoint");
        let lat = geo
            .and_then(|g| g.get("latitudeInMicroDegrees"))
            .and_then(parse_i64_value);
        let lng = geo
            .and_then(|g| g.get("longitudeInMicroDegrees"))
            .and_then(parse_i64_value);
        let radius = prox.get("radius").and_then(parse_f64_value);
        let units = prox
            .get("radiusUnits")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && *s != "UNSPECIFIED" && *s != "UNKNOWN");
        if let (Some(lat), Some(lng), Some(radius), Some(units)) = (lat, lng, radius, units) {
            target.proximity = Some(JsonProximity {
                latitude: lat as f64 / 1_000_000.0,
                longitude: lng as f64 / 1_000_000.0,
                radius,
                radius_units: units.to_string(),
            });
        }
    }
    if let Some(t) = nested_enum(v, "device", "type") {
        target.device = Some(JsonDevice { ty: t });
    }
    if let Some(id) = nested_str(v, "youtubeChannel", "channelId") {
        target.youtube_channel = Some(JsonYoutubeChannel { channel_id: id });
    }
    if let Some(id) = nested_str(v, "youtubeVideo", "videoId") {
        target.youtube_video = Some(JsonYoutubeVideo { video_id: id });
    }
    if let Some(c) = nested_str(v, "topic", "topicConstant") {
        target.topic = Some(JsonTopic { topic_constant: c });
    }
    if let Some(url) = nested_str(v, "placement", "url") {
        target.placement = Some(JsonPlacement { url });
    }
    if let Some(c) = nested_str(v, "userInterest", "userInterestCategory") {
        target.user_interest = Some(JsonUserInterest {
            user_interest_category: c,
        });
    }
    if let Some(t) = nested_enum(v, "ageRange", "type") {
        target.age_range = Some(JsonAgeRange { ty: t });
    }
    if let Some(t) = nested_enum(v, "gender", "type") {
        target.gender = Some(JsonGender { ty: t });
    }
    if let Some(t) = nested_enum(v, "parentalStatus", "type") {
        target.parental_status = Some(JsonParentalStatus { ty: t });
    }
    if let Some(t) = nested_enum(v, "incomeRange", "type") {
        target.income_range = Some(JsonIncomeRange { ty: t });
    }
    for (message, field) in [
        ("customAudience", "customAudience"),
        ("userList", "userList"),
        ("combinedAudience", "combinedAudience"),
    ] {
        let Some(rn) = nested_str(v, message, field) else {
            continue;
        };
        target.audience = Some(match message {
            "customAudience" => JsonAudience {
                custom_audience: Some(rn),
                user_list: None,
                combined_audience: None,
            },
            "userList" => JsonAudience {
                custom_audience: None,
                user_list: Some(rn),
                combined_audience: None,
            },
            _ => JsonAudience {
                custom_audience: None,
                user_list: None,
                combined_audience: Some(rn),
            },
        });
    }
}

fn nested_str(v: &Value, message: &str, field: &str) -> Option<String> {
    v.get(message)?
        .get(field)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Google returns `UNSPECIFIED` / `UNKNOWN` for an enum it declines to name;
/// neither is a value a `.bid` may declare, so treat them as absent.
fn nested_enum(v: &Value, message: &str, field: &str) -> Option<String> {
    nested_str(v, message, field).filter(|s| s != "UNSPECIFIED" && s != "UNKNOWN")
}

fn parse_custom_audience_member(v: &Value) -> Option<JsonCustomAudienceMember> {
    let m = JsonCustomAudienceMember {
        keyword: v.get("keyword").and_then(Value::as_str).map(str::to_string),
        url: v.get("url").and_then(Value::as_str).map(str::to_string),
        place_category: v
            .get("placeCategory")
            .and_then(Value::as_str)
            .map(str::to_string),
        app: v.get("app").and_then(Value::as_str).map(str::to_string),
    };
    m.payload().is_some().then_some(m)
}

fn parse_i64(v: Option<&Value>) -> Option<i64> {
    parse_i64_value(v?)
}

fn parse_i64_value(v: &Value) -> Option<i64> {
    if let Some(n) = v.as_i64() {
        return Some(n);
    }
    if let Some(s) = v.as_str() {
        return s.parse::<i64>().ok();
    }
    None
}

fn parse_f64_value(v: &Value) -> Option<f64> {
    if let Some(n) = v.as_f64() {
        return Some(n);
    }
    if let Some(s) = v.as_str() {
        return s.parse::<f64>().ok();
    }
    None
}

/// Extract a list of ad-text asset strings (headlines / descriptions / CTAs on a
/// demand-gen ad). Each item is an `AdTextAsset { text }` or a bare string.
fn extract_ad_text_list(v: Option<&Value>) -> Vec<String> {
    let Some(arr) = v.and_then(Value::as_array) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|item| {
            item.as_str().map(String::from).or_else(|| {
                item.get("text").and_then(Value::as_str).map(String::from)
            })
        })
        .collect()
}

/// Extract youtube-video asset ids from a demand-gen ad's `videos` array. Each
/// item is an `AdVideoAsset { asset: "customers/x/assets/ID" }`.
fn extract_video_asset_ids(v: Option<&Value>) -> Vec<String> {
    let Some(arr) = v.and_then(Value::as_array) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|item| {
            item.get("asset")
                .and_then(Value::as_str)
                .and_then(last_segment)
                .map(String::from)
        })
        .collect()
}

fn extract_rsa_assets(v: Option<&Value>) -> Vec<JsonRsaAsset> {
    let Some(arr) = v.and_then(Value::as_array) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|item| {
            let text = item.as_str().map(String::from).or_else(|| {
                item.get("text")
                    .and_then(Value::as_str)
                    .map(String::from)
            })?;
            let pin = item
                .get("pinnedField")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty() && *s != "UNSPECIFIED" && *s != "UNKNOWN")
                .map(String::from);
            Some(JsonRsaAsset { text, pin })
        })
        .collect()
}

#[cfg(test)]
mod bidding_tests {
    use super::*;

    fn adapt(rows: Value) -> ExportInput {
        from_search_response(&Value::Object(
            [("results".to_string(), rows)].into_iter().collect(),
        ).to_string())
        .expect("adapter should succeed")
    }

    #[test]
    fn a_live_impression_share_strategy_reads_its_subfields() {
        let input = adapt(serde_json::json!([
            {
                "campaign": {
                    "resourceName": "customers/123/campaigns/555",
                    "id": "555",
                    "name": "Search_Generic",
                    "advertisingChannelType": "SEARCH",
                    "campaignBudget": "customers/123/campaignBudgets/999",
                    "biddingStrategyType": "TARGET_IMPRESSION_SHARE",
                    "targetImpressionShare": {
                        "location": "ANYWHERE_ON_PAGE",
                        "locationFractionMicros": "800000",
                        "cpcBidCeilingMicros": "500000"
                    }
                }
            }
        ]));
        let c = &input.campaigns[0];
        assert_eq!(c.bidding_strategy(), Some("target_impression_share"));
        let tis = c.target_impression_share.as_ref().expect("tis read");
        assert_eq!(tis.location.as_deref(), Some("ANYWHERE_ON_PAGE"));
        assert_eq!(tis.location_fraction_micros, Some(800000));
        assert_eq!(tis.cpc_bid_ceiling_micros, Some(500000));
    }

    #[test]
    fn a_live_target_spend_reads_its_ceiling() {
        let input = adapt(serde_json::json!([
            {
                "campaign": {
                    "resourceName": "customers/123/campaigns/556",
                    "id": "556",
                    "name": "Search_uBlock",
                    "advertisingChannelType": "SEARCH",
                    "campaignBudget": "customers/123/campaignBudgets/999",
                    "biddingStrategyType": "TARGET_SPEND",
                    "targetSpend": { "cpcBidCeilingMicros": "1100000" }
                }
            }
        ]));
        let c = &input.campaigns[0];
        assert_eq!(c.bidding_strategy(), Some("target_spend"));
        assert_eq!(
            c.target_spend.as_ref().and_then(|t| t.cpc_bid_ceiling_micros),
            Some(1100000)
        );
    }

    /// A strategy whose every field is unset comes back with no message object
    /// at all, leaving `bidding_strategy_type` as the only tell.
    #[test]
    fn an_uncapped_target_spend_still_reads_as_the_strategy() {
        let input = adapt(serde_json::json!([
            {
                "campaign": {
                    "resourceName": "customers/123/campaigns/557",
                    "id": "557",
                    "name": "Search_Uncapped",
                    "advertisingChannelType": "SEARCH",
                    "campaignBudget": "customers/123/campaignBudgets/999",
                    "biddingStrategyType": "TARGET_SPEND"
                }
            }
        ]));
        let c = &input.campaigns[0];
        assert_eq!(c.bidding_strategy(), Some("target_spend"));
        assert_eq!(
            c.target_spend.as_ref().and_then(|t| t.cpc_bid_ceiling_micros),
            None
        );
    }
}

#[cfg(test)]
mod label_tests {
    use super::*;

    fn adapt(rows: Value) -> ExportInput {
        from_search_response(&Value::Object(
            [("results".to_string(), rows)].into_iter().collect(),
        ).to_string())
        .expect("adapter should succeed")
    }

    #[test]
    fn campaign_label_populates_managed_address() {
        let input = adapt(serde_json::json!([
            {
                "campaign": {
                    "resourceName": "customers/123/campaigns/555",
                    "id": "555",
                    "name": "Summer",
                    "advertisingChannelType": "SEARCH",
                    "campaignBudget": "customers/123/campaignBudgets/999"
                }
            },
            {
                "campaignLabel": {
                    "resourceName": "customers/123/campaignLabels/555~777",
                    "campaign": "customers/123/campaigns/555",
                    "label": "customers/123/labels/777"
                },
                "label": {
                    "resourceName": "customers/123/labels/777",
                    "name": "bidsmith:address=main.google_ads_campaign.summer"
                }
            }
        ]));

        let campaign = input.campaigns.iter().find(|c| c.id == "555").expect("campaign present");
        assert_eq!(
            campaign.managed_address.as_deref(),
            Some("main.google_ads_campaign.summer")
        );
    }

    #[test]
    fn composite_criterion_label_maps_to_the_criterion() {
        let input = adapt(serde_json::json!([
            {
                "adGroupCriterion": {
                    "resourceName": "customers/123/adGroupCriteria/300~400",
                    "adGroup": "customers/123/adGroups/300",
                    "keyword": { "text": "shoes", "matchType": "BROAD" }
                }
            },
            {
                "adGroupCriterionLabel": {
                    "resourceName": "customers/123/adGroupCriterionLabels/300~400~777",
                    "adGroupCriterion": "customers/123/adGroupCriteria/300~400",
                    "label": "customers/123/labels/777"
                },
                "label": { "name": "bidsmith:address=main.google_ads_ad_group_criterion.shoes" }
            }
        ]));

        let crit = input
            .ad_group_criteria
            .iter()
            .find(|c| c.id == "300~400")
            .expect("criterion present");
        assert_eq!(
            crit.managed_address.as_deref(),
            Some("main.google_ads_ad_group_criterion.shoes")
        );
    }

    #[test]
    fn owns_labels_populate_claims_and_claim_labels() {
        let input = adapt(serde_json::json!([
            {
                "adGroup": {
                    "resourceName": "customers/123/adGroups/300",
                    "id": "300",
                    "name": "G",
                    "campaign": "customers/123/campaigns/100"
                }
            },
            {
                "adGroupLabel": {
                    "resourceName": "customers/123/adGroupLabels/300~777",
                    "adGroup": "customers/123/adGroups/300",
                    "label": "customers/123/labels/777"
                },
                "label": {
                    "resourceName": "customers/123/labels/777",
                    "name": "bidsmith:owns=keyword_negative"
                }
            },
            {
                "campaignLabel": {
                    "resourceName": "customers/123/campaignLabels/100~778",
                    "campaign": "customers/123/campaigns/100",
                    "label": "customers/123/labels/778"
                },
                "label": {
                    "resourceName": "customers/123/labels/778",
                    "name": "bidsmith:owns=location"
                }
            }
        ]));

        assert_eq!(
            input.ad_group_claims.get("300"),
            Some(&vec!["keyword_negative".to_string()])
        );
        assert_eq!(
            input.campaign_claims.get("100"),
            Some(&vec!["location".to_string()])
        );
        assert_eq!(
            input.claim_labels.get("keyword_negative").map(String::as_str),
            Some("customers/123/labels/777")
        );
        assert_eq!(
            input.claim_labels.get("location").map(String::as_str),
            Some("customers/123/labels/778")
        );
        assert!(input.labels.is_empty(), "owns labels must not pollute address labels");
    }

    #[test]
    fn non_bidsmith_label_is_ignored() {
        let input = adapt(serde_json::json!([
            {
                "campaign": {
                    "resourceName": "customers/123/campaigns/555",
                    "id": "555",
                    "name": "Summer",
                    "advertisingChannelType": "SEARCH",
                    "campaignBudget": "customers/123/campaignBudgets/999"
                }
            },
            {
                "campaignLabel": {
                    "campaign": "customers/123/campaigns/555",
                    "label": "customers/123/labels/888"
                },
                "label": { "name": "Q4 Promo" }
            }
        ]));

        let campaign = input.campaigns.iter().find(|c| c.id == "555").expect("campaign present");
        assert_eq!(campaign.managed_address, None);
    }
}


#[cfg(test)]
mod video_tests {
    use super::*;

    fn adapt(rows: Value) -> ExportInput {
        from_search_response(
            &Value::Object([("results".to_string(), rows)].into_iter().collect()).to_string(),
        )
        .expect("adapter should succeed")
    }

    #[test]
    fn video_responsive_ad_reads_back_with_its_asset_id() {
        let input = adapt(serde_json::json!([
            {
                "adGroupAd": {
                    "resourceName": "customers/123/adGroupAds/55~9",
                    "adGroup": "customers/123/adGroups/55",
                    "status": "PAUSED",
                    "ad": {
                        "finalUrls": ["https://ghostery.com/get"],
                        "videoResponsiveAd": {
                            "headlines": [{"text": "Block ads"}],
                            "longHeadlines": [{"text": "Block ads and trackers"}],
                            "descriptions": [{"text": "Free extension"}],
                            "callToActions": [{"text": "Install"}],
                            "videos": [{"asset": "customers/123/assets/42"}]
                        }
                    }
                }
            }
        ]));

        let ad = input.ad_group_ads.first().expect("the ad is adapted");
        let video = ad.ad.video_responsive_ad.as_ref().expect("video creative");
        assert_eq!(video.video, "42");
        assert_eq!(video.headlines, vec!["Block ads"]);
        assert_eq!(video.long_headlines, vec!["Block ads and trackers"]);
        assert_eq!(video.descriptions, vec!["Free extension"]);
        assert_eq!(video.call_to_actions, vec!["Install"]);
    }

    #[test]
    fn a_ui_built_video_ad_reads_back_whole() {
        // The shape a live in-stream creative actually has: `Ad.video_ad` with
        // one asset, and the tracking URL the campaign is measured on beside it.
        let input = adapt(serde_json::json!([
            {
                "adGroupAd": {
                    "resourceName": "customers/123/adGroupAds/55~9",
                    "adGroup": "customers/123/adGroups/55",
                    "status": "ENABLED",
                    "ad": {
                        "finalUrls": ["https://ghostery.com/?utm_campaign=GH_YouTubeUS_v1"],
                        "finalMobileUrls": ["https://m.ghostery.com/?utm_campaign=GH_YouTubeUS_v1"],
                        "displayUrl": "www.ghostery.com",
                        "videoAd": {
                            "video": {"asset": "customers/123/assets/75804823141"}
                        }
                    }
                }
            }
        ]));

        let ad = input.ad_group_ads.first().expect("the ad is adapted");
        assert_eq!(ad.ad.display_url.as_deref(), Some("www.ghostery.com"));
        assert_eq!(
            ad.ad.final_mobile_urls,
            vec!["https://m.ghostery.com/?utm_campaign=GH_YouTubeUS_v1"]
        );
        let video = ad.ad.video_ad.as_ref().expect("video_ad creative");
        assert_eq!(video.video, "75804823141");
    }

    #[test]
    fn video_responsive_breadcrumbs_read_back() {
        let input = adapt(serde_json::json!([
            {
                "adGroupAd": {
                    "resourceName": "customers/123/adGroupAds/55~9",
                    "adGroup": "customers/123/adGroups/55",
                    "ad": {
                        "finalUrls": ["https://ghostery.com/get"],
                        "videoResponsiveAd": {
                            "headlines": [{"text": "Block ads"}],
                            "videos": [{"asset": "customers/123/assets/42"}],
                            "breadcrumb1": "AdBlocker",
                            "breadcrumb2": "Browser"
                        }
                    }
                }
            }
        ]));

        let video = input.ad_group_ads[0].ad.video_responsive_ad.as_ref().expect("creative");
        assert_eq!(video.breadcrumb1.as_deref(), Some("AdBlocker"));
        assert_eq!(video.breadcrumb2.as_deref(), Some("Browser"));
    }

    #[test]
    fn demand_gen_business_name_reads_back_from_its_text_asset() {
        let input = adapt(serde_json::json!([
            {
                "adGroupAd": {
                    "resourceName": "customers/123/adGroupAds/55~9",
                    "adGroup": "customers/123/adGroups/55",
                    "ad": {
                        "finalUrls": ["https://ghostery.com/get"],
                        "demandGenVideoResponsiveAd": {
                            "headlines": [{"text": "Block ads"}],
                            "businessName": {"text": "Ghostery"},
                            "videos": [{"asset": "customers/123/assets/42"}]
                        }
                    }
                }
            }
        ]));

        let ad = input.ad_group_ads.first().expect("the ad is adapted");
        let dg = ad
            .ad
            .demand_gen_video_responsive_ad
            .as_ref()
            .expect("demand gen creative");
        assert_eq!(dg.business_name.as_deref(), Some("Ghostery"));
        assert_eq!(dg.videos, vec!["42"]);
    }

    #[test]
    fn ad_group_targeting_reads_back_off_the_criterion_row() {
        let input = adapt(serde_json::json!([
            {
                "adGroupCriterion": {
                    "resourceName": "customers/123/adGroupCriteria/55~1",
                    "adGroup": "customers/123/adGroups/55",
                    "status": "ENABLED",
                    "bidModifier": 1.2,
                    "ageRange": {"type": "AGE_RANGE_35_44"}
                }
            },
            {
                "adGroupCriterion": {
                    "resourceName": "customers/123/adGroupCriteria/55~2",
                    "adGroup": "customers/123/adGroups/55",
                    "negative": true,
                    "placement": {"url": "https://example.com/x"}
                }
            },
            {
                "adGroupCriterion": {
                    "resourceName": "customers/123/adGroupCriteria/55~3",
                    "adGroup": "customers/123/adGroups/55",
                    "userList": {"userList": "customers/123/userLists/987"}
                }
            },
            {
                "adGroupCriterion": {
                    "resourceName": "customers/123/adGroupCriteria/55~4",
                    "adGroup": "customers/123/adGroups/55",
                    "mobileApplication": {"appId": "1-com.example"}
                }
            }
        ]));

        let by_id = |id: &str| {
            input
                .ad_group_criteria
                .iter()
                .find(|c| c.id == id)
                .unwrap_or_else(|| panic!("criterion {id}"))
        };
        let age = by_id("55~1");
        assert_eq!(
            age.target.age_range.as_ref().map(|a| a.ty.as_str()),
            Some("AGE_RANGE_35_44")
        );
        assert_eq!(age.bid_modifier, Some(1.2));
        assert_eq!(
            by_id("55~2").target.placement.as_ref().map(|p| p.url.as_str()),
            Some("https://example.com/x")
        );
        assert_eq!(
            by_id("55~3")
                .target
                .audience
                .as_ref()
                .and_then(|a| a.user_list.as_deref()),
            Some("customers/123/userLists/987")
        );
        assert!(
            !input.ad_group_criteria.iter().any(|c| c.id == "55~4"),
            "a criterion type bidsmith cannot render must not adapt into an empty resource"
        );
    }
}
