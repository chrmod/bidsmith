use std::collections::BTreeMap;

use serde_json::Value;

use crate::commands::export::{
    ExportInput, JsonAd, JsonAdGroup, JsonAdGroupAd, JsonAdGroupAsset, JsonAdGroupCriterion,
    JsonBudget, JsonCallAsset, JsonCalloutAsset, JsonCampaign, JsonCampaignAsset,
    JsonCampaignCriterion, JsonCampaignSharedSet, JsonConversionAction, JsonCustomerAsset,
    JsonDemandGenVideoResponsiveAd, JsonDevice, JsonKeyword, JsonLanguage, JsonLocation,
    JsonManualCpc, JsonNetworkSettings, JsonProximity, JsonResponsiveSearchAd, JsonRsaAsset,
    JsonSharedSet, JsonSitelinkAsset, JsonStructuredSnippetAsset, JsonValueSettings,
    JsonYoutubeVideoAsset,
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
    // value: (real criterion resource-name segment `<setId>~<critId>`, keyword)
    shared_criteria: BTreeMap<String, Vec<(String, JsonKeyword)>>,
    campaign_shared_sets: BTreeMap<String, JsonCampaignSharedSet>,
    // live entity id -> bidsmith:address, read from label associations.
    campaign_addresses: BTreeMap<String, String>,
    ad_group_addresses: BTreeMap<String, String>,
    ad_group_ad_addresses: BTreeMap<String, String>,
    ad_group_criterion_addresses: BTreeMap<String, String>,
    // bidsmith:address -> label resource_name, read from the label resource.
    labels: BTreeMap<String, String>,
}

impl AdapterState {
    fn absorb_row(&mut self, row: &Value) {
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
        }
        if let Some(v) = row.get("adGroupLabel") {
            self.merge_label(v, "adGroup", label, |s| &mut s.ad_group_addresses);
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

    /// Record a `bidsmith:address=<addr>` label resource (address -> its
    /// resource_name) so the mutate builder can reuse an existing label rather
    /// than re-create one. Rows from the association queries select only
    /// `label.name` (no resource_name), so they no-op here; the standalone
    /// `label` query carries both.
    fn merge_label_resource(&mut self, label: &Value) {
        let Some(address) = label_address(Some(label)) else {
            return;
        };
        if let Some(rn) = label.get("resourceName").and_then(Value::as_str) {
            self.labels.insert(address, rn.to_string());
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
            amount_micros: 0,
            delivery_method: None,
            explicitly_shared: None,
        });
        if let Some(s) = v.get("name").and_then(Value::as_str) {
            entry.name = s.to_string();
        }
        if let Some(n) = parse_i64(v.get("amountMicros")) {
            entry.amount_micros = n;
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
                campaign_budget: String::new(),
                contains_eu_political_advertising: None,
                manual_cpc: None,
                network_settings: None,
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
        }
        if let Some(ns) = v.get("networkSettings") {
            entry.network_settings = Some(JsonNetworkSettings {
                target_google_search: ns.get("targetGoogleSearch").and_then(Value::as_bool),
                target_search_network: ns.get("targetSearchNetwork").and_then(Value::as_bool),
                target_content_network: ns.get("targetContentNetwork").and_then(Value::as_bool),
                target_partner_search_network: ns
                    .get("targetPartnerSearchNetwork")
                    .and_then(Value::as_bool),
            });
        }
    }

    fn merge_ad_group(&mut self, v: &Value) {
        if let Some(rn) = v.get("resourceName").and_then(Value::as_str) {
            self.note_customer(rn);
        }
        let Some(id) = extract_id(v) else { return };
        let entry = self.ad_groups.entry(id.clone()).or_insert_with(|| JsonAdGroup {
            id,
            name: String::new(),
            campaign: String::new(),
            status: None,
            ty: None,
            cpc_bid_micros: None,
            managed_address: None,
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
        if let Some(n) = parse_i64(v.get("cpcBidMicros")) {
            entry.cpc_bid_micros = Some(n);
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
                    responsive_search_ad: None,
                    video_responsive_ad: None,
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
            if let Some(urls) = ad.get("finalUrls").and_then(Value::as_array) {
                let urls: Vec<String> = urls
                    .iter()
                    .filter_map(|u| u.as_str().map(String::from))
                    .collect();
                if !urls.is_empty() {
                    entry.ad.final_urls = urls;
                }
            }
            if let Some(rsa) = ad.get("responsiveSearchAd") {
                entry.ad.responsive_search_ad = Some(JsonResponsiveSearchAd {
                    headlines: extract_rsa_assets(rsa.get("headlines")),
                    descriptions: extract_rsa_assets(rsa.get("descriptions")),
                    path1: rsa.get("path1").and_then(Value::as_str).map(String::from),
                    path2: rsa.get("path2").and_then(Value::as_str).map(String::from),
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
        let Some(kw_value) = v.get("keyword") else {
            return;
        };
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
                keyword: JsonKeyword {
                    text: String::new(),
                    match_type: String::new(),
                },
                managed_address: None,
            });
        if !ad_group_id.is_empty() {
            entry.ad_group = ad_group_id;
        }
        entry.keyword = JsonKeyword {
            text: kw_value
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            match_type: kw_value
                .get("matchType")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        };
        if let Some(s) = v.get("status").and_then(Value::as_str) {
            entry.status = Some(s.to_string());
        }
        if let Some(b) = v.get("negative").and_then(Value::as_bool) {
            entry.negative = Some(b);
        }
        if let Some(n) = parse_i64(v.get("cpcBidMicros")) {
            entry.cpc_bid_micros = Some(n);
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
                keyword: None,
                location: None,
                language: None,
                proximity: None,
                device: None,
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
        if let Some(dev) = v.get("device") {
            if let Some(t) = dev
                .get("type")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty() && *s != "UNSPECIFIED" && *s != "UNKNOWN")
            {
                entry.device = Some(JsonDevice { ty: t.to_string() });
            }
        }
        if let Some(kw) = v.get("keyword") {
            entry.keyword = Some(JsonKeyword {
                text: kw.get("text").and_then(Value::as_str).unwrap_or("").to_string(),
                match_type: kw
                    .get("matchType")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            });
        }
        if let Some(loc) = v.get("location") {
            if let Some(s) = loc.get("geoTargetConstant").and_then(Value::as_str) {
                entry.location = Some(JsonLocation {
                    geo_target_constant: s.to_string(),
                });
            }
        }
        if let Some(lang) = v.get("language") {
            if let Some(s) = lang.get("languageConstant").and_then(Value::as_str) {
                entry.language = Some(JsonLanguage {
                    language_constant: s.to_string(),
                });
            }
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
            if let (Some(lat), Some(lng), Some(radius), Some(units)) =
                (lat, lng, radius, units)
            {
                entry.proximity = Some(JsonProximity {
                    latitude: lat as f64 / 1_000_000.0,
                    longitude: lng as f64 / 1_000_000.0,
                    radius,
                    radius_units: units.to_string(),
                });
            }
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
            labels: self.labels.into_iter().collect(),
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

