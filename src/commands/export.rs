use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::process::ExitCode;

use serde::Deserialize;

#[derive(Deserialize)]
pub struct ExportInput {
    pub customer_id: String,
    #[serde(default)]
    pub login_customer_id: Option<String>,
    #[serde(default)]
    pub campaign_budgets: Vec<JsonBudget>,
    #[serde(default)]
    pub campaigns: Vec<JsonCampaign>,
    #[serde(default)]
    pub ad_groups: Vec<JsonAdGroup>,
    #[serde(default)]
    pub ad_group_ads: Vec<JsonAdGroupAd>,
    #[serde(default)]
    pub ad_group_criteria: Vec<JsonAdGroupCriterion>,
    #[serde(default)]
    pub campaign_criteria: Vec<JsonCampaignCriterion>,
    #[serde(default)]
    pub conversion_actions: Vec<JsonConversionAction>,
    #[serde(default)]
    pub call_assets: Vec<JsonCallAsset>,
    #[serde(default)]
    pub customer_assets: Vec<JsonCustomerAsset>,
    #[serde(default)]
    pub shared_sets: Vec<JsonSharedSet>,
    #[serde(default)]
    pub campaign_shared_sets: Vec<JsonCampaignSharedSet>,
}

#[derive(Deserialize)]
pub struct JsonBudget {
    pub id: String,
    pub name: String,
    pub amount_micros: i64,
    #[serde(default)]
    pub delivery_method: Option<String>,
    #[serde(default)]
    pub explicitly_shared: Option<bool>,
}

#[derive(Deserialize)]
pub struct JsonCampaign {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub status: Option<String>,
    pub advertising_channel_type: String,
    pub campaign_budget: String,
    #[serde(default)]
    pub contains_eu_political_advertising: Option<String>,
    #[serde(default)]
    pub manual_cpc: Option<JsonManualCpc>,
    #[serde(default)]
    pub network_settings: Option<JsonNetworkSettings>,
}

#[derive(Deserialize)]
pub struct JsonManualCpc {
    #[serde(default)]
    pub enhanced_cpc_enabled: Option<bool>,
}

#[derive(Deserialize)]
pub struct JsonNetworkSettings {
    #[serde(default)]
    pub target_google_search: Option<bool>,
    #[serde(default)]
    pub target_search_network: Option<bool>,
    #[serde(default)]
    pub target_content_network: Option<bool>,
    #[serde(default)]
    pub target_partner_search_network: Option<bool>,
}

#[derive(Deserialize)]
pub struct JsonAdGroup {
    pub id: String,
    pub name: String,
    pub campaign: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default, rename = "type")]
    pub ty: Option<String>,
    #[serde(default)]
    pub cpc_bid_micros: Option<i64>,
}

#[derive(Deserialize)]
pub struct JsonAdGroupAd {
    #[allow(dead_code)]
    pub id: String,
    pub ad_group: String,
    #[serde(default)]
    pub status: Option<String>,
    pub ad: JsonAd,
}

#[derive(Deserialize)]
pub struct JsonAd {
    #[serde(default)]
    pub name: Option<String>,
    pub final_urls: Vec<String>,
    #[serde(default)]
    pub responsive_search_ad: Option<JsonResponsiveSearchAd>,
}

#[derive(Deserialize)]
pub struct JsonAdGroupCriterion {
    #[allow(dead_code)]
    pub id: String,
    pub ad_group: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub negative: Option<bool>,
    #[serde(default)]
    pub cpc_bid_micros: Option<i64>,
    pub keyword: JsonKeyword,
}

#[derive(Deserialize)]
pub struct JsonCampaignCriterion {
    pub id: String,
    pub campaign: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub negative: Option<bool>,
    #[serde(default)]
    pub keyword: Option<JsonKeyword>,
    #[serde(default)]
    pub location: Option<JsonLocation>,
    #[serde(default)]
    pub language: Option<JsonLanguage>,
    #[serde(default)]
    pub proximity: Option<JsonProximity>,
}

#[derive(Deserialize)]
pub struct JsonKeyword {
    pub text: String,
    pub match_type: String,
}

#[derive(Deserialize)]
pub struct JsonLocation {
    pub geo_target_constant: String,
}

#[derive(Deserialize)]
pub struct JsonLanguage {
    pub language_constant: String,
}

#[derive(Deserialize)]
pub struct JsonProximity {
    pub latitude: f64,
    pub longitude: f64,
    pub radius: f64,
    pub radius_units: String,
}

#[derive(Deserialize)]
pub struct JsonConversionAction {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub category: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub counting_type: Option<String>,
    #[serde(default)]
    pub click_through_lookback_window_days: Option<i64>,
    #[serde(default)]
    pub view_through_lookback_window_days: Option<i64>,
    #[serde(default)]
    pub value_settings: Option<JsonValueSettings>,
}

#[derive(Deserialize)]
pub struct JsonValueSettings {
    #[serde(default)]
    pub default_value: Option<f64>,
    #[serde(default)]
    pub default_currency_code: Option<String>,
    #[serde(default)]
    pub always_use_default_value: Option<bool>,
}

#[derive(Deserialize)]
pub struct JsonCallAsset {
    pub id: String,
    pub country_code: String,
    pub phone_number: String,
    #[serde(default)]
    pub call_conversion_reporting_state: Option<String>,
    #[serde(default)]
    pub call_conversion_action: Option<String>,
}

#[derive(Deserialize)]
pub struct JsonCustomerAsset {
    pub id: String,
    pub asset: String,
    pub field_type: String,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Deserialize)]
pub struct JsonSharedSet {
    pub id: String,
    pub name: String,
    #[serde(default, rename = "type")]
    pub ty: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub negative_keywords: Vec<JsonKeyword>,
}

#[derive(Deserialize)]
pub struct JsonCampaignSharedSet {
    pub id: String,
    pub campaign: String,
    pub shared_set: String,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Deserialize)]
pub struct JsonResponsiveSearchAd {
    pub headlines: Vec<JsonRsaAsset>,
    pub descriptions: Vec<JsonRsaAsset>,
    #[serde(default)]
    pub path1: Option<String>,
    #[serde(default)]
    pub path2: Option<String>,
}

#[derive(Deserialize)]
pub struct JsonRsaAsset {
    pub text: String,
    #[serde(default)]
    pub pin: Option<String>,
}

pub fn run(
    from_json: Option<&str>,
    from_gads_search_response: Option<&str>,
    output: Option<&str>,
    include_removed: bool,
    login_customer_id_override: Option<&str>,
    customer_id_override: Option<&str>,
) -> ExitCode {
    let mut input = match (from_json, from_gads_search_response) {
        (Some(path), None) => match load_flat_json(path) {
            Ok(v) => v,
            Err(code) => return code,
        },
        (None, Some(path)) => match load_gads_search_response(path) {
            Ok(v) => v,
            Err(code) => return code,
        },
        (None, None) => {
            eprintln!(
                "export: provide --from-json <PATH> or --from-gads-search-response <PATH>"
            );
            return ExitCode::from(2);
        }
        (Some(_), Some(_)) => {
            eprintln!("export: --from-json and --from-gads-search-response are mutually exclusive");
            return ExitCode::from(2);
        }
    };

    let customer_id_resolved = customer_id_override
        .map(str::to_string)
        .or_else(|| std::env::var("GOOGLE_ADS_CUSTOMER_ID").ok())
        .filter(|s| !s.is_empty());
    if let Some(id) = customer_id_resolved {
        input.customer_id = id;
    }
    let login_customer_id_resolved = login_customer_id_override
        .map(str::to_string)
        .or_else(|| std::env::var("GOOGLE_ADS_LOGIN_CUSTOMER_ID").ok())
        .filter(|s| !s.is_empty());
    if let Some(id) = login_customer_id_resolved {
        input.login_customer_id = Some(id);
    }
    if !include_removed {
        filter_removed(&mut input);
    }

    let rendered = canonicalize(&render(&input));

    match output {
        None => {
            print!("{rendered}");
            ExitCode::SUCCESS
        }
        Some(path) => match std::fs::write(path, &rendered) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("failed to write {path}: {e}");
                ExitCode::from(1)
            }
        },
    }
}

pub fn canonicalize(raw: &str) -> String {
    match raw.parse::<hcl_edit::structure::Body>() {
        Ok(body) => crate::commands::fmt::format_body(&body),
        Err(_) => raw.to_string(),
    }
}

pub fn filter_removed(input: &mut ExportInput) {
    let is_removed = |s: &Option<String>| s.as_deref() == Some("REMOVED");
    input.campaigns.retain(|c| !is_removed(&c.status));
    input.ad_groups.retain(|g| !is_removed(&g.status));
    input.ad_group_ads.retain(|a| !is_removed(&a.status));
    input.ad_group_criteria.retain(|c| !is_removed(&c.status));
    input.campaign_criteria.retain(|c| !is_removed(&c.status));
    input
        .conversion_actions
        .retain(|c| !is_removed(&c.status));
    input.customer_assets.retain(|a| !is_removed(&a.status));
    input.shared_sets.retain(|s| !is_removed(&s.status));
    input
        .campaign_shared_sets
        .retain(|s| !is_removed(&s.status));
}

fn load_flat_json(path: &str) -> Result<ExportInput, ExitCode> {
    let raw = std::fs::read_to_string(path).map_err(|e| {
        eprintln!("failed to read {path}: {e}");
        ExitCode::from(1)
    })?;
    serde_json::from_str(&raw).map_err(|e| {
        eprintln!("failed to parse {path}: {e}");
        ExitCode::from(1)
    })
}

fn load_gads_search_response(path: &str) -> Result<ExportInput, ExitCode> {
    let raw = std::fs::read_to_string(path).map_err(|e| {
        eprintln!("failed to read {path}: {e}");
        ExitCode::from(1)
    })?;
    crate::commands::adapt::from_search_response(&raw).map_err(|e| {
        eprintln!("failed to adapt {path}: {e}");
        ExitCode::from(1)
    })
}

pub fn render_split(input: &ExportInput) -> (String, String) {
    let mut account = String::new();
    let mut campaigns = String::new();
    let mut names = NameAllocator::default();

    let mut budget_addr: HashMap<String, String> = HashMap::new();
    let mut campaign_addr: HashMap<String, String> = HashMap::new();
    let mut ad_group_addr: HashMap<String, String> = HashMap::new();
    let mut conversion_action_addr: HashMap<String, String> = HashMap::new();
    let mut call_asset_addr: HashMap<String, String> = HashMap::new();
    let mut shared_set_addr: HashMap<String, String> = HashMap::new();

    write_provider(&mut account, input);

    for c in &input.conversion_actions {
        let name = names.allocate("google_ads_conversion_action", &slugify(&c.name));
        conversion_action_addr
            .insert(c.id.clone(), format!("google_ads_conversion_action.{name}"));
        write_conversion_action(&mut account, &name, c);
    }

    for a in &input.call_assets {
        let base = format!("call_{}_{}", a.country_code, a.phone_number);
        let name = names.allocate("google_ads_call_asset", &slugify(&base));
        call_asset_addr.insert(a.id.clone(), format!("google_ads_call_asset.{name}"));
        write_call_asset(&mut account, &name, a, &conversion_action_addr);
    }

    for a in &input.customer_assets {
        let base = call_asset_addr
            .get(&a.asset)
            .and_then(|addr| addr.strip_prefix("google_ads_call_asset."))
            .map(|s| format!("link_{s}"))
            .unwrap_or_else(|| slugify(&a.id));
        let name = names.allocate("google_ads_customer_asset", &slugify(&base));
        write_customer_asset(&mut account, &name, a, &call_asset_addr);
    }

    for s in &input.shared_sets {
        let name = names.allocate("google_ads_shared_set", &slugify(&s.name));
        shared_set_addr.insert(s.id.clone(), format!("google_ads_shared_set.{name}"));
        write_shared_set(&mut account, &name, s);
    }

    let has_campaign_resources = !input.campaign_budgets.is_empty()
        || !input.campaigns.is_empty()
        || !input.ad_groups.is_empty()
        || !input.ad_group_ads.is_empty()
        || !input.ad_group_criteria.is_empty()
        || !input.campaign_criteria.is_empty()
        || !input.campaign_shared_sets.is_empty();

    if has_campaign_resources {
        write_provider(&mut campaigns, input);

        for b in &input.campaign_budgets {
            let name = names.allocate("google_ads_campaign_budget", &slugify(&b.name));
            budget_addr.insert(b.id.clone(), format!("google_ads_campaign_budget.{name}"));
            write_budget(&mut campaigns, &name, b);
        }

        for c in &input.campaigns {
            let name = names.allocate("google_ads_campaign", &slugify(&c.name));
            campaign_addr.insert(c.id.clone(), format!("google_ads_campaign.{name}"));
            write_campaign(&mut campaigns, &name, c, &budget_addr);
        }

        for g in &input.ad_groups {
            let name = names.allocate("google_ads_ad_group", &slugify(&g.name));
            ad_group_addr.insert(g.id.clone(), format!("google_ads_ad_group.{name}"));
            write_ad_group(&mut campaigns, &name, g, &campaign_addr);
        }

        for a in &input.ad_group_ads {
            let base = ad_ad_base(a, &ad_group_addr);
            let name = names.allocate("google_ads_ad_group_ad", &base);
            write_ad_group_ad(&mut campaigns, &name, a, &ad_group_addr);
        }

        for group in group_ad_group_criteria(&input.ad_group_criteria) {
            let base = ad_group_criterion_group_base(&group, &ad_group_addr);
            let name = names.allocate("google_ads_ad_group_criterion", &slugify(&base));
            write_ad_group_criterion_group(&mut campaigns, &name, &group, &ad_group_addr);
        }

        let (negative_groups, singletons) = partition_campaign_criteria(&input.campaign_criteria);
        for group in negative_groups {
            let base = campaign_negative_group_base(&group, &campaign_addr);
            let name = names.allocate("google_ads_campaign_criterion", &slugify(&base));
            write_campaign_negative_group(&mut campaigns, &name, &group, &campaign_addr);
        }
        for c in singletons {
            let base = criterion_base(c);
            let name = names.allocate("google_ads_campaign_criterion", &slugify(&base));
            write_campaign_criterion(&mut campaigns, &name, c, &campaign_addr);
        }

        for s in &input.campaign_shared_sets {
            let base = match (
                campaign_addr
                    .get(&s.campaign)
                    .and_then(|a| a.strip_prefix("google_ads_campaign.")),
                shared_set_addr
                    .get(&s.shared_set)
                    .and_then(|a| a.strip_prefix("google_ads_shared_set.")),
            ) {
                (Some(c), Some(ss)) => format!("{c}_{ss}"),
                _ => slugify(&s.id),
            };
            let name = names.allocate("google_ads_campaign_shared_set", &slugify(&base));
            write_campaign_shared_set(&mut campaigns, &name, s, &campaign_addr, &shared_set_addr);
        }
    }

    while account.ends_with("\n\n\n") {
        account.pop();
    }
    while campaigns.ends_with("\n\n\n") {
        campaigns.pop();
    }

    (account, campaigns)
}

fn render(input: &ExportInput) -> String {
    let mut out = String::new();
    let mut names = NameAllocator::default();

    let mut budget_addr: HashMap<String, String> = HashMap::new();
    let mut campaign_addr: HashMap<String, String> = HashMap::new();
    let mut ad_group_addr: HashMap<String, String> = HashMap::new();
    let mut conversion_action_addr: HashMap<String, String> = HashMap::new();
    let mut call_asset_addr: HashMap<String, String> = HashMap::new();
    let mut shared_set_addr: HashMap<String, String> = HashMap::new();

    write_provider(&mut out, input);

    for c in &input.conversion_actions {
        let name = names.allocate("google_ads_conversion_action", &slugify(&c.name));
        conversion_action_addr
            .insert(c.id.clone(), format!("google_ads_conversion_action.{name}"));
        write_conversion_action(&mut out, &name, c);
    }

    for a in &input.call_assets {
        let base = format!("call_{}_{}", a.country_code, a.phone_number);
        let name = names.allocate("google_ads_call_asset", &slugify(&base));
        call_asset_addr.insert(a.id.clone(), format!("google_ads_call_asset.{name}"));
        write_call_asset(&mut out, &name, a, &conversion_action_addr);
    }

    for a in &input.customer_assets {
        let base = call_asset_addr
            .get(&a.asset)
            .and_then(|addr| addr.strip_prefix("google_ads_call_asset."))
            .map(|s| format!("link_{s}"))
            .unwrap_or_else(|| slugify(&a.id));
        let name = names.allocate("google_ads_customer_asset", &slugify(&base));
        write_customer_asset(&mut out, &name, a, &call_asset_addr);
    }

    for b in &input.campaign_budgets {
        let name = names.allocate("google_ads_campaign_budget", &slugify(&b.name));
        budget_addr.insert(b.id.clone(), format!("google_ads_campaign_budget.{name}"));
        write_budget(&mut out, &name, b);
    }

    for c in &input.campaigns {
        let name = names.allocate("google_ads_campaign", &slugify(&c.name));
        campaign_addr.insert(c.id.clone(), format!("google_ads_campaign.{name}"));
        write_campaign(&mut out, &name, c, &budget_addr);
    }

    for g in &input.ad_groups {
        let name = names.allocate("google_ads_ad_group", &slugify(&g.name));
        ad_group_addr.insert(g.id.clone(), format!("google_ads_ad_group.{name}"));
        write_ad_group(&mut out, &name, g, &campaign_addr);
    }

    for a in &input.ad_group_ads {
        let base = ad_ad_base(a, &ad_group_addr);
        let name = names.allocate("google_ads_ad_group_ad", &base);
        write_ad_group_ad(&mut out, &name, a, &ad_group_addr);
    }

    for group in group_ad_group_criteria(&input.ad_group_criteria) {
        let base = ad_group_criterion_group_base(&group, &ad_group_addr);
        let name = names.allocate("google_ads_ad_group_criterion", &slugify(&base));
        write_ad_group_criterion_group(&mut out, &name, &group, &ad_group_addr);
    }

    let (negative_groups, singletons) = partition_campaign_criteria(&input.campaign_criteria);
    for group in negative_groups {
        let base = campaign_negative_group_base(&group, &campaign_addr);
        let name = names.allocate("google_ads_campaign_criterion", &slugify(&base));
        write_campaign_negative_group(&mut out, &name, &group, &campaign_addr);
    }
    for c in singletons {
        let base = criterion_base(c);
        let name = names.allocate("google_ads_campaign_criterion", &slugify(&base));
        write_campaign_criterion(&mut out, &name, c, &campaign_addr);
    }

    for s in &input.shared_sets {
        let name = names.allocate("google_ads_shared_set", &slugify(&s.name));
        shared_set_addr.insert(s.id.clone(), format!("google_ads_shared_set.{name}"));
        write_shared_set(&mut out, &name, s);
    }

    for s in &input.campaign_shared_sets {
        let base = match (
            campaign_addr
                .get(&s.campaign)
                .and_then(|a| a.strip_prefix("google_ads_campaign.")),
            shared_set_addr
                .get(&s.shared_set)
                .and_then(|a| a.strip_prefix("google_ads_shared_set.")),
        ) {
            (Some(c), Some(ss)) => format!("{c}_{ss}"),
            _ => slugify(&s.id),
        };
        let name = names.allocate("google_ads_campaign_shared_set", &slugify(&base));
        write_campaign_shared_set(&mut out, &name, s, &campaign_addr, &shared_set_addr);
    }

    while out.ends_with("\n\n\n") {
        out.pop();
    }
    out
}

fn write_provider(out: &mut String, input: &ExportInput) {
    out.push_str("provider \"google_ads\" {\n");
    write_attr(out, 1, "customer_id", &fmt_string(&input.customer_id));
    if let Some(lc) = &input.login_customer_id {
        write_attr(out, 1, "login_customer_id", &fmt_string(lc));
    }
    out.push_str("}\n\n");
}

fn write_budget(out: &mut String, name: &str, b: &JsonBudget) {
    let _ = writeln!(out, "resource \"google_ads_campaign_budget\" \"{name}\" {{");
    write_attr(out, 1, "name", &fmt_string(&b.name));
    write_attr(out, 1, "amount_micros", &b.amount_micros.to_string());
    if let Some(dm) = &b.delivery_method {
        write_attr(out, 1, "delivery_method", &fmt_string(dm));
    }
    if let Some(es) = b.explicitly_shared {
        write_attr(out, 1, "explicitly_shared", &es.to_string());
    }
    out.push_str("}\n\n");
}

fn write_campaign(
    out: &mut String,
    name: &str,
    c: &JsonCampaign,
    budget_addr: &HashMap<String, String>,
) {
    let _ = writeln!(out, "resource \"google_ads_campaign\" \"{name}\" {{");
    write_attr(out, 1, "name", &fmt_string(&c.name));
    if let Some(s) = &c.status {
        write_attr(out, 1, "status", &fmt_string(s));
    }
    write_attr(
        out,
        1,
        "advertising_channel_type",
        &fmt_string(&c.advertising_channel_type),
    );
    let budget_ref = match budget_addr.get(&c.campaign_budget) {
        Some(addr) => format!("{addr}.id"),
        None => format!("\"<unresolved budget {}>\"", c.campaign_budget),
    };
    write_attr(out, 1, "campaign_budget", &budget_ref);
    if let Some(v) = &c.contains_eu_political_advertising {
        write_attr(out, 1, "contains_eu_political_advertising", &fmt_string(v));
    }

    if let Some(m) = &c.manual_cpc {
        out.push_str("\n  manual_cpc {\n");
        if let Some(e) = m.enhanced_cpc_enabled {
            write_attr(out, 2, "enhanced_cpc_enabled", &e.to_string());
        }
        out.push_str("  }\n");
    }
    if let Some(n) = &c.network_settings {
        out.push_str("\n  network_settings {\n");
        for (k, v) in [
            ("target_google_search", n.target_google_search),
            ("target_search_network", n.target_search_network),
            ("target_content_network", n.target_content_network),
            ("target_partner_search_network", n.target_partner_search_network),
        ] {
            if let Some(v) = v {
                write_attr(out, 2, k, &v.to_string());
            }
        }
        out.push_str("  }\n");
    }
    out.push_str("}\n\n");
}

fn write_ad_group(
    out: &mut String,
    name: &str,
    g: &JsonAdGroup,
    campaign_addr: &HashMap<String, String>,
) {
    let _ = writeln!(out, "resource \"google_ads_ad_group\" \"{name}\" {{");
    write_attr(out, 1, "name", &fmt_string(&g.name));
    let campaign_ref = match campaign_addr.get(&g.campaign) {
        Some(addr) => format!("{addr}.id"),
        None => format!("\"<unresolved campaign {}>\"", g.campaign),
    };
    write_attr(out, 1, "campaign", &campaign_ref);
    if let Some(s) = &g.status {
        write_attr(out, 1, "status", &fmt_string(s));
    }
    if let Some(t) = &g.ty {
        write_attr(out, 1, "type", &fmt_string(t));
    }
    if let Some(c) = g.cpc_bid_micros {
        write_attr(out, 1, "cpc_bid_micros", &c.to_string());
    }
    out.push_str("}\n\n");
}

fn write_ad_group_ad(
    out: &mut String,
    name: &str,
    a: &JsonAdGroupAd,
    ad_group_addr: &HashMap<String, String>,
) {
    let _ = writeln!(out, "resource \"google_ads_ad_group_ad\" \"{name}\" {{");
    let ad_group_ref = match ad_group_addr.get(&a.ad_group) {
        Some(addr) => format!("{addr}.id"),
        None => format!("\"<unresolved ad_group {}>\"", a.ad_group),
    };
    write_attr(out, 1, "ad_group", &ad_group_ref);
    if let Some(s) = &a.status {
        write_attr(out, 1, "status", &fmt_string(s));
    }

    out.push_str("\n  ad {\n");
    if let Some(n) = &a.ad.name {
        write_attr(out, 2, "name", &fmt_string(n));
    }
    write_attr(out, 2, "final_urls", &fmt_string_list(&a.ad.final_urls));
    if let Some(rsa) = &a.ad.responsive_search_ad {
        out.push_str("\n    responsive_search_ad {\n");
        if !rsa.headlines.is_empty() {
            write_attr(out, 3, "headlines", &fmt_rsa_asset_list(&rsa.headlines));
        }
        if !rsa.descriptions.is_empty() {
            write_attr(out, 3, "descriptions", &fmt_rsa_asset_list(&rsa.descriptions));
        }
        if let Some(p) = &rsa.path1 {
            write_attr(out, 3, "path1", &fmt_string(p));
        }
        if let Some(p) = &rsa.path2 {
            write_attr(out, 3, "path2", &fmt_string(p));
        }
        out.push_str("    }\n");
    }
    out.push_str("  }\n");
    out.push_str("}\n\n");
}

type AdGroupCriterionKey = (String, bool, Option<String>, Option<i64>, Option<String>);

fn group_ad_group_criteria(
    items: &[JsonAdGroupCriterion],
) -> Vec<Vec<&JsonAdGroupCriterion>> {
    let mut groups: Vec<Vec<&JsonAdGroupCriterion>> = Vec::new();
    let mut index: HashMap<AdGroupCriterionKey, usize> = HashMap::new();
    for c in items {
        let neg = c.negative.unwrap_or(false);
        let match_type_key = if neg {
            None
        } else {
            Some(c.keyword.match_type.clone())
        };
        let key = (
            c.ad_group.clone(),
            neg,
            c.status.clone(),
            c.cpc_bid_micros,
            match_type_key,
        );
        let idx = match index.get(&key) {
            Some(&i) => i,
            None => {
                let i = groups.len();
                index.insert(key, i);
                groups.push(Vec::new());
                i
            }
        };
        groups[idx].push(c);
    }
    groups
}

fn ad_group_criterion_group_base(
    group: &[&JsonAdGroupCriterion],
    ad_group_addr: &HashMap<String, String>,
) -> String {
    let first = group[0];
    let ag_slug = ad_group_addr
        .get(&first.ad_group)
        .and_then(|s| s.strip_prefix("google_ads_ad_group."))
        .unwrap_or(&first.ad_group);
    if first.negative.unwrap_or(false) {
        format!("{ag_slug}_negatives")
    } else {
        format!(
            "{ag_slug}_{}",
            first.keyword.match_type.to_ascii_lowercase()
        )
    }
}

fn write_ad_group_criterion_group(
    out: &mut String,
    name: &str,
    group: &[&JsonAdGroupCriterion],
    ad_group_addr: &HashMap<String, String>,
) {
    let first = group[0];
    let _ = writeln!(
        out,
        "resource \"google_ads_ad_group_criterion\" \"{name}\" {{"
    );
    let ag_ref = match ad_group_addr.get(&first.ad_group) {
        Some(addr) => format!("{addr}.id"),
        None => format!("\"<unresolved ad_group {}>\"", first.ad_group),
    };
    write_attr(out, 1, "ad_group", &ag_ref);
    if let Some(s) = &first.status {
        write_attr(out, 1, "status", &fmt_string(s));
    }
    if let Some(cpc) = first.cpc_bid_micros {
        write_attr(out, 1, "cpc_bid_micros", &cpc.to_string());
    }
    let block_name = if first.negative.unwrap_or(false) {
        "negative_keyword"
    } else {
        "keyword"
    };
    for c in group {
        out.push('\n');
        let _ = writeln!(out, "  {block_name} {{");
        write_attr(out, 2, "text", &fmt_string(&c.keyword.text));
        write_attr(out, 2, "match_type", &fmt_string(&c.keyword.match_type));
        out.push_str("  }\n");
    }
    out.push_str("}\n\n");
}

fn partition_campaign_criteria(
    items: &[JsonCampaignCriterion],
) -> (Vec<Vec<&JsonCampaignCriterion>>, Vec<&JsonCampaignCriterion>) {
    let mut groups: Vec<Vec<&JsonCampaignCriterion>> = Vec::new();
    let mut index: HashMap<(String, Option<String>), usize> = HashMap::new();
    let mut singletons: Vec<&JsonCampaignCriterion> = Vec::new();
    for c in items {
        let is_negative_keyword = c.negative.unwrap_or(false) && c.keyword.is_some();
        if is_negative_keyword {
            let key = (c.campaign.clone(), c.status.clone());
            let idx = match index.get(&key) {
                Some(&i) => i,
                None => {
                    let i = groups.len();
                    index.insert(key, i);
                    groups.push(Vec::new());
                    i
                }
            };
            groups[idx].push(c);
        } else {
            singletons.push(c);
        }
    }
    (groups, singletons)
}

fn campaign_negative_group_base(
    group: &[&JsonCampaignCriterion],
    campaign_addr: &HashMap<String, String>,
) -> String {
    let first = group[0];
    let camp_slug = campaign_addr
        .get(&first.campaign)
        .and_then(|s| s.strip_prefix("google_ads_campaign."))
        .unwrap_or(&first.campaign);
    format!("{camp_slug}_negatives")
}

fn write_campaign_negative_group(
    out: &mut String,
    name: &str,
    group: &[&JsonCampaignCriterion],
    campaign_addr: &HashMap<String, String>,
) {
    let first = group[0];
    let _ = writeln!(
        out,
        "resource \"google_ads_campaign_criterion\" \"{name}\" {{"
    );
    let camp_ref = match campaign_addr.get(&first.campaign) {
        Some(addr) => format!("{addr}.id"),
        None => format!("\"<unresolved campaign {}>\"", first.campaign),
    };
    write_attr(out, 1, "campaign", &camp_ref);
    if let Some(s) = &first.status {
        write_attr(out, 1, "status", &fmt_string(s));
    }
    for c in group {
        if let Some(kw) = &c.keyword {
            out.push_str("\n  negative_keyword {\n");
            write_attr(out, 2, "text", &fmt_string(&kw.text));
            write_attr(out, 2, "match_type", &fmt_string(&kw.match_type));
            out.push_str("  }\n");
        }
    }
    out.push_str("}\n\n");
}

fn write_campaign_criterion(
    out: &mut String,
    name: &str,
    c: &JsonCampaignCriterion,
    campaign_addr: &HashMap<String, String>,
) {
    let _ = writeln!(
        out,
        "resource \"google_ads_campaign_criterion\" \"{name}\" {{"
    );
    let camp_ref = match campaign_addr.get(&c.campaign) {
        Some(addr) => format!("{addr}.id"),
        None => format!("\"<unresolved campaign {}>\"", c.campaign),
    };
    write_attr(out, 1, "campaign", &camp_ref);
    if let Some(s) = &c.status {
        write_attr(out, 1, "status", &fmt_string(s));
    }
    if let Some(kw) = &c.keyword {
        write_keyword(out, kw);
    }
    if let Some(loc) = &c.location {
        out.push_str("\n  location {\n");
        write_attr(
            out,
            2,
            "geo_target_constant",
            &fmt_string(&loc.geo_target_constant),
        );
        out.push_str("  }\n");
    }
    if let Some(lang) = &c.language {
        out.push_str("\n  language {\n");
        write_attr(
            out,
            2,
            "language_constant",
            &fmt_string(&lang.language_constant),
        );
        out.push_str("  }\n");
    }
    if let Some(prox) = &c.proximity {
        out.push_str("\n  proximity {\n");
        write_attr(out, 2, "latitude", &format_number(prox.latitude));
        write_attr(out, 2, "longitude", &format_number(prox.longitude));
        write_attr(out, 2, "radius", &format_number(prox.radius));
        write_attr(out, 2, "radius_units", &fmt_string(&prox.radius_units));
        out.push_str("  }\n");
    }
    out.push_str("}\n\n");
}

fn write_conversion_action(out: &mut String, name: &str, c: &JsonConversionAction) {
    let _ = writeln!(out, "resource \"google_ads_conversion_action\" \"{name}\" {{");
    write_attr(out, 1, "name", &fmt_string(&c.name));
    write_attr(out, 1, "type", &fmt_string(&c.ty));
    write_attr(out, 1, "category", &fmt_string(&c.category));
    if let Some(s) = &c.status {
        write_attr(out, 1, "status", &fmt_string(s));
    }
    if let Some(ct) = &c.counting_type {
        write_attr(out, 1, "counting_type", &fmt_string(ct));
    }
    if let Some(d) = c.click_through_lookback_window_days {
        write_attr(out, 1, "click_through_lookback_window_days", &d.to_string());
    }
    if let Some(d) = c.view_through_lookback_window_days {
        write_attr(out, 1, "view_through_lookback_window_days", &d.to_string());
    }
    if let Some(vs) = &c.value_settings {
        out.push_str("\n  value_settings {\n");
        if let Some(v) = vs.default_value {
            write_attr(out, 2, "default_value", &format_number(v));
        }
        if let Some(s) = &vs.default_currency_code {
            write_attr(out, 2, "default_currency_code", &fmt_string(s));
        }
        if let Some(b) = vs.always_use_default_value {
            write_attr(out, 2, "always_use_default_value", &b.to_string());
        }
        out.push_str("  }\n");
    }
    out.push_str("}\n\n");
}

fn write_call_asset(
    out: &mut String,
    name: &str,
    a: &JsonCallAsset,
    conversion_action_addr: &HashMap<String, String>,
) {
    let _ = writeln!(out, "resource \"google_ads_call_asset\" \"{name}\" {{");
    write_attr(out, 1, "country_code", &fmt_string(&a.country_code));
    write_attr(out, 1, "phone_number", &fmt_string(&a.phone_number));
    if let Some(s) = &a.call_conversion_reporting_state {
        write_attr(out, 1, "call_conversion_reporting_state", &fmt_string(s));
    }
    if let Some(action) = &a.call_conversion_action {
        let action_ref = match conversion_action_addr.get(action) {
            Some(addr) => format!("{addr}.id"),
            None => format!("\"<unresolved conversion_action {action}>\""),
        };
        write_attr(out, 1, "call_conversion_action", &action_ref);
    }
    out.push_str("}\n\n");
}

fn write_customer_asset(
    out: &mut String,
    name: &str,
    a: &JsonCustomerAsset,
    call_asset_addr: &HashMap<String, String>,
) {
    let _ = writeln!(out, "resource \"google_ads_customer_asset\" \"{name}\" {{");
    let asset_ref = match call_asset_addr.get(&a.asset) {
        Some(addr) => format!("{addr}.id"),
        None => format!("\"<unresolved asset {}>\"", a.asset),
    };
    write_attr(out, 1, "asset", &asset_ref);
    write_attr(out, 1, "field_type", &fmt_string(&a.field_type));
    if let Some(s) = &a.status {
        write_attr(out, 1, "status", &fmt_string(s));
    }
    out.push_str("}\n\n");
}

fn write_shared_set(out: &mut String, name: &str, s: &JsonSharedSet) {
    let _ = writeln!(out, "resource \"google_ads_shared_set\" \"{name}\" {{");
    write_attr(out, 1, "name", &fmt_string(&s.name));
    if let Some(t) = &s.ty {
        write_attr(out, 1, "type", &fmt_string(t));
    }
    if let Some(st) = &s.status {
        write_attr(out, 1, "status", &fmt_string(st));
    }
    for kw in &s.negative_keywords {
        out.push_str("\n  negative_keyword {\n");
        write_attr(out, 2, "text", &fmt_string(&kw.text));
        write_attr(out, 2, "match_type", &fmt_string(&kw.match_type));
        out.push_str("  }\n");
    }
    out.push_str("}\n\n");
}

fn write_campaign_shared_set(
    out: &mut String,
    name: &str,
    s: &JsonCampaignSharedSet,
    campaign_addr: &HashMap<String, String>,
    shared_set_addr: &HashMap<String, String>,
) {
    let _ = writeln!(
        out,
        "resource \"google_ads_campaign_shared_set\" \"{name}\" {{"
    );
    let campaign_ref = match campaign_addr.get(&s.campaign) {
        Some(addr) => format!("{addr}.id"),
        None => format!("\"<unresolved campaign {}>\"", s.campaign),
    };
    write_attr(out, 1, "campaign", &campaign_ref);
    let shared_set_ref = match shared_set_addr.get(&s.shared_set) {
        Some(addr) => format!("{addr}.id"),
        None => format!("\"<unresolved shared_set {}>\"", s.shared_set),
    };
    write_attr(out, 1, "shared_set", &shared_set_ref);
    if let Some(st) = &s.status {
        write_attr(out, 1, "status", &fmt_string(st));
    }
    out.push_str("}\n\n");
}

fn format_number(n: f64) -> String {
    if n.fract() == 0.0 && n.is_finite() {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

fn write_keyword(out: &mut String, kw: &JsonKeyword) {
    out.push_str("\n  keyword {\n");
    write_attr(out, 2, "text", &fmt_string(&kw.text));
    write_attr(out, 2, "match_type", &fmt_string(&kw.match_type));
    out.push_str("  }\n");
}

fn ad_ad_base(a: &JsonAdGroupAd, ad_group_addr: &HashMap<String, String>) -> String {
    if let Some(name) = a
        .ad
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return slugify(name);
    }
    if let Some(slug) = ad_group_addr
        .get(&a.ad_group)
        .and_then(|s| s.strip_prefix("google_ads_ad_group."))
        .filter(|s| !s.is_empty())
    {
        let suffix = if a.ad.responsive_search_ad.is_some() {
            "rsa"
        } else {
            "ad"
        };
        return format!("{slug}_{suffix}");
    }
    slugify(&a.id)
}

fn criterion_base(c: &JsonCampaignCriterion) -> String {
    if let Some(kw) = &c.keyword {
        return format!("{}_{}", kw.match_type.to_ascii_lowercase(), kw.text);
    }
    if let Some(loc) = &c.location {
        return format!("location_{}", loc.geo_target_constant);
    }
    if let Some(lang) = &c.language {
        return format!("language_{}", lang.language_constant);
    }
    if let Some(prox) = &c.proximity {
        return format!(
            "proximity_{}_{}",
            prox.radius_units.to_ascii_lowercase(),
            format_number(prox.radius),
        );
    }
    c.id.clone()
}

fn fmt_rsa_asset_list(assets: &[JsonRsaAsset]) -> String {
    let mut out = String::from("[");
    for (i, asset) in assets.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        match &asset.pin {
            None => out.push_str(&fmt_string(&asset.text)),
            Some(pin) => {
                out.push_str("{ text = ");
                out.push_str(&fmt_string(&asset.text));
                out.push_str(", pin = ");
                out.push_str(&fmt_string(pin));
                out.push_str(" }");
            }
        }
    }
    out.push(']');
    out
}

fn fmt_string_list(items: &[String]) -> String {
    let mut out = String::from("[");
    for (i, s) in items.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&fmt_string(s));
    }
    out.push(']');
    out
}

fn write_attr(out: &mut String, indent: usize, name: &str, value: &str) {
    for _ in 0..indent {
        out.push_str("  ");
    }
    let _ = writeln!(out, "{name} = {value}");
}

fn fmt_string(s: &str) -> String {
    let mut escaped = String::with_capacity(s.len() + 2);
    escaped.push('"');
    for ch in s.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            c => escaped.push(c),
        }
    }
    escaped.push('"');
    escaped
}

fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut prev_underscore = true;
    for ch in s.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            out.push(lower);
            prev_underscore = false;
        } else if !prev_underscore {
            out.push('_');
            prev_underscore = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.is_empty() {
        out.push_str("resource");
    }
    if out
        .chars()
        .next()
        .map(|c| c.is_ascii_digit())
        .unwrap_or(false)
    {
        out.insert(0, '_');
    }
    out
}

#[derive(Default)]
struct NameAllocator {
    by_type: HashMap<&'static str, HashSet<String>>,
}

impl NameAllocator {
    fn allocate(&mut self, resource_type: &'static str, base: &str) -> String {
        let set = self.by_type.entry(resource_type).or_default();
        if set.insert(base.to_string()) {
            return base.to_string();
        }
        let mut i = 2;
        loop {
            let candidate = format!("{base}_{i}");
            if set.insert(candidate.clone()) {
                return candidate;
            }
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::adapt::from_search_response;

    const FULL_FIXTURE: &str = r#"[
        {
            "results": [
                { "campaignBudget": { "resourceName": "customers/9/campaignBudgets/1001", "id": "1001", "name": "Budget A", "amountMicros": "5000000" } },
                { "campaign": { "resourceName": "customers/9/campaigns/2001", "id": "2001", "name": "Camp A", "status": "ENABLED", "advertisingChannelType": "SEARCH", "campaignBudget": "customers/9/campaignBudgets/1001" } },
                { "conversionAction": { "resourceName": "customers/9/conversionActions/3001", "id": "3001", "name": "Lead", "type": "WEBPAGE", "category": "SUBMIT_LEAD_FORM", "status": "ENABLED" } },
                { "asset": { "resourceName": "customers/9/assets/4001", "id": "4001", "callAsset": { "countryCode": "PL", "phoneNumber": "510019081" } } },
                { "customerAsset": { "resourceName": "customers/9/customerAssets/4001~CALL", "asset": "customers/9/assets/4001", "fieldType": "CALL", "status": "ENABLED" } }
            ]
        }
    ]"#;

    #[test]
    fn render_split_separates_account_and_campaign_buckets() {
        let input = from_search_response(FULL_FIXTURE).expect("adapter");
        let (account, campaigns) = render_split(&input);

        assert!(account.contains("google_ads_conversion_action"));
        assert!(account.contains("google_ads_call_asset"));
        assert!(account.contains("google_ads_customer_asset"));
        assert!(!account.contains("resource \"google_ads_campaign\""));
        assert!(!account.contains("google_ads_campaign_budget"));

        assert!(campaigns.contains("resource \"google_ads_campaign\""));
        assert!(campaigns.contains("google_ads_campaign_budget"));
        assert!(!campaigns.contains("google_ads_conversion_action"));
        assert!(!campaigns.contains("google_ads_call_asset"));
        assert!(!campaigns.contains("google_ads_customer_asset"));

        assert!(account.starts_with("provider \"google_ads\""));
        assert!(campaigns.starts_with("provider \"google_ads\""));
    }

    #[test]
    fn render_split_account_only_produces_no_campaigns_file() {
        let raw = r#"[
            {
                "results": [
                    { "conversionAction": { "resourceName": "customers/9/conversionActions/3001", "id": "3001", "name": "Lead", "type": "WEBPAGE", "category": "SUBMIT_LEAD_FORM" } }
                ]
            }
        ]"#;
        let input = from_search_response(raw).unwrap();
        let (account, campaigns) = render_split(&input);
        assert!(account.contains("google_ads_conversion_action"));
        assert!(campaigns.is_empty(), "campaigns should be empty, got {campaigns:?}");
    }

    #[test]
    fn render_split_emits_parseable_hcl2() {
        let input = from_search_response(FULL_FIXTURE).expect("adapter");
        let (account, campaigns) = render_split(&input);
        let _: hcl_edit::structure::Body =
            account.parse().expect("account.bid parses as HCL2");
        let _: hcl_edit::structure::Body =
            campaigns.parse().expect("campaigns.bid parses as HCL2");
    }

    #[test]
    fn render_split_validates_as_a_directory() {
        let raw = r#"[
            {
                "results": [
                    { "campaignBudget": { "resourceName": "customers/9/campaignBudgets/1001", "id": "1001", "name": "Budget A", "amountMicros": "5000000" } },
                    { "campaign": { "resourceName": "customers/9/campaigns/2001", "id": "2001", "name": "Camp A", "status": "ENABLED", "advertisingChannelType": "SEARCH", "campaignBudget": "customers/9/campaignBudgets/1001", "containsEuPoliticalAdvertising": "DOES_NOT_CONTAIN_EU_POLITICAL_ADVERTISING" } },
                    { "sharedSet": { "resourceName": "customers/9/sharedSets/9001", "id": "9001", "name": "negs", "type": "NEGATIVE_KEYWORDS", "status": "ENABLED" } },
                    { "campaignSharedSet": { "resourceName": "customers/9/campaignSharedSets/2001~9001", "campaign": "customers/9/campaigns/2001", "sharedSet": "customers/9/sharedSets/9001", "status": "ENABLED" } }
                ]
            }
        ]"#;
        let input = from_search_response(raw).expect("adapter");
        let (account, campaigns) = render_split(&input);

        let dir = std::env::temp_dir()
            .join(format!("bidsmith-rs-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let account_path = dir.join("account.bid");
        let campaigns_path = dir.join("campaigns.bid");
        std::fs::write(&account_path, &account).unwrap();
        std::fs::write(&campaigns_path, &campaigns).unwrap();

        let parsed = vec![
            crate::parser::parse_file(&account_path).expect("account parses"),
            crate::parser::parse_file(&campaigns_path).expect("campaigns parses"),
        ];
        let diags = crate::schema::validate_files(&parsed);
        let errors: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
        assert!(
            errors.is_empty(),
            "validate_files produced errors:\n{}\n--- account.bid ---\n{}\n--- campaigns.bid ---\n{}",
            errors
                .iter()
                .map(|d| format!("{}", d.message))
                .collect::<Vec<_>>()
                .join("\n"),
            account,
            campaigns
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
