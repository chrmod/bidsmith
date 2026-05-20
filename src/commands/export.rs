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

fn canonicalize(raw: &str) -> String {
    match raw.parse::<hcl_edit::structure::Body>() {
        Ok(body) => crate::commands::fmt::format_body(&body),
        Err(_) => raw.to_string(),
    }
}

fn filter_removed(input: &mut ExportInput) {
    let is_removed = |s: &Option<String>| s.as_deref() == Some("REMOVED");
    input.campaigns.retain(|c| !is_removed(&c.status));
    input.ad_groups.retain(|g| !is_removed(&g.status));
    input.ad_group_ads.retain(|a| !is_removed(&a.status));
    input.ad_group_criteria.retain(|c| !is_removed(&c.status));
    input.campaign_criteria.retain(|c| !is_removed(&c.status));
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

fn render(input: &ExportInput) -> String {
    let mut out = String::new();
    let mut names = NameAllocator::default();

    let mut budget_addr: HashMap<String, String> = HashMap::new();
    let mut campaign_addr: HashMap<String, String> = HashMap::new();
    let mut ad_group_addr: HashMap<String, String> = HashMap::new();

    write_provider(&mut out, input);

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

    for c in &input.ad_group_criteria {
        let base = format!(
            "{}_{}",
            c.keyword.match_type.to_ascii_lowercase(),
            c.keyword.text
        );
        let name = names.allocate("google_ads_ad_group_criterion", &slugify(&base));
        write_ad_group_criterion(&mut out, &name, c, &ad_group_addr);
    }

    for c in &input.campaign_criteria {
        let base = criterion_base(c);
        let name = names.allocate("google_ads_campaign_criterion", &slugify(&base));
        write_campaign_criterion(&mut out, &name, c, &campaign_addr);
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
        for (i, h) in rsa.headlines.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            write_rsa_asset(out, 3, "headline", h);
        }
        if !rsa.headlines.is_empty() && !rsa.descriptions.is_empty() {
            out.push('\n');
        }
        for (i, d) in rsa.descriptions.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            write_rsa_asset(out, 3, "description", d);
        }
        if rsa.path1.is_some() || rsa.path2.is_some() {
            if !rsa.headlines.is_empty() || !rsa.descriptions.is_empty() {
                out.push('\n');
            }
            if let Some(p) = &rsa.path1 {
                write_attr(out, 3, "path1", &fmt_string(p));
            }
            if let Some(p) = &rsa.path2 {
                write_attr(out, 3, "path2", &fmt_string(p));
            }
        }
        out.push_str("    }\n");
    }
    out.push_str("  }\n");
    out.push_str("}\n\n");
}

fn write_ad_group_criterion(
    out: &mut String,
    name: &str,
    c: &JsonAdGroupCriterion,
    ad_group_addr: &HashMap<String, String>,
) {
    let _ = writeln!(
        out,
        "resource \"google_ads_ad_group_criterion\" \"{name}\" {{"
    );
    let ag_ref = match ad_group_addr.get(&c.ad_group) {
        Some(addr) => format!("{addr}.id"),
        None => format!("\"<unresolved ad_group {}>\"", c.ad_group),
    };
    write_attr(out, 1, "ad_group", &ag_ref);
    if let Some(s) = &c.status {
        write_attr(out, 1, "status", &fmt_string(s));
    }
    if let Some(neg) = c.negative {
        write_attr(out, 1, "negative", &neg.to_string());
    }
    if let Some(cpc) = c.cpc_bid_micros {
        write_attr(out, 1, "cpc_bid_micros", &cpc.to_string());
    }
    write_keyword(out, &c.keyword);
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
    if let Some(neg) = c.negative {
        write_attr(out, 1, "negative", &neg.to_string());
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

fn write_rsa_asset(out: &mut String, indent: usize, kind: &str, asset: &JsonRsaAsset) {
    for _ in 0..indent {
        out.push_str("  ");
    }
    let _ = writeln!(out, "{kind} {{");
    write_attr(out, indent + 1, "text", &fmt_string(&asset.text));
    if let Some(p) = &asset.pin {
        write_attr(out, indent + 1, "pin", &fmt_string(p));
    }
    for _ in 0..indent {
        out.push_str("  ");
    }
    out.push_str("}\n");
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
