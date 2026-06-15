use std::collections::HashMap;

use crate::commands::export::{
    ExportInput, JsonAdGroup, JsonAdGroupAd, JsonAdGroupCriterion, JsonBudget, JsonCallAsset,
    JsonCampaign, JsonCampaignCriterion, JsonCampaignSharedSet, JsonConversionAction,
    JsonCustomerAsset, JsonSharedCriterion, JsonSharedSet,
};

#[derive(Debug, Clone)]
pub enum Action {
    NoOp {
        live_id: String,
    },
    Create,
    Update {
        live_id: String,
        changed_fields: Vec<String>,
    },
    /// A live resource that is no longer declared and should be removed. Only
    /// emitted for criteria members whose declared parent still exists, so the
    /// parent scopes the pruning (no `bidsmith:address` labels needed).
    Delete {
        live_id: String,
    },
}

impl Action {
    pub fn live_id(&self) -> Option<&str> {
        match self {
            Action::NoOp { live_id }
            | Action::Update { live_id, .. }
            | Action::Delete { live_id } => Some(live_id.as_str()),
            Action::Create => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResourceDiff {
    pub address: String,
    pub kind: &'static str,
    pub action: Action,
}

pub struct DiffReport {
    pub diffs: Vec<ResourceDiff>,
    pub noop_count: usize,
    pub create_count: usize,
    pub update_count: usize,
    pub delete_count: usize,
}

pub fn diff(declared: &ExportInput, live: &ExportInput) -> DiffReport {
    let mut diffs: Vec<ResourceDiff> = Vec::new();
    let mut campaign_match: HashMap<String, String> = HashMap::new();
    let mut ad_group_match: HashMap<String, String> = HashMap::new();
    let mut conversion_match: HashMap<String, String> = HashMap::new();
    let mut call_asset_match: HashMap<String, String> = HashMap::new();

    // ---- campaign_budgets (match by name) --------------------------------
    let live_budgets: HashMap<&str, &JsonBudget> = live
        .campaign_budgets
        .iter()
        .map(|b| (b.name.as_str(), b))
        .collect();
    for d in &declared.campaign_budgets {
        let action = match live_budgets.get(d.name.as_str()) {
            Some(l) => action_for_match(l.id.clone(), diff_budget(d, l)),
            None => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "campaign_budget",
            action,
        });
    }

    // ---- campaigns (match by name) ---------------------------------------
    let live_campaigns: HashMap<&str, &JsonCampaign> = live
        .campaigns
        .iter()
        .map(|c| (c.name.as_str(), c))
        .collect();
    for d in &declared.campaigns {
        let action = match live_campaigns.get(d.name.as_str()) {
            Some(l) => {
                campaign_match.insert(d.id.clone(), l.id.clone());
                action_for_match(l.id.clone(), diff_campaign(d, l))
            }
            None => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "campaign",
            action,
        });
    }

    // ---- ad_groups (match by mapped_campaign_id + name) ------------------
    let live_ad_groups: HashMap<(String, String), &JsonAdGroup> = live
        .ad_groups
        .iter()
        .map(|g| ((g.campaign.clone(), g.name.clone()), g))
        .collect();
    for d in &declared.ad_groups {
        let action = match campaign_match.get(&d.campaign) {
            Some(parent_id) => match live_ad_groups.get(&(parent_id.clone(), d.name.clone())) {
                Some(l) => {
                    ad_group_match.insert(d.id.clone(), l.id.clone());
                    action_for_match(l.id.clone(), diff_ad_group(d, l))
                }
                None => Action::Create,
            },
            None => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "ad_group",
            action,
        });
    }

    // ---- ad_group_ads (match by mapped_ad_group_id, first ad) ------------
    let mut live_ads_by_ag: HashMap<String, &JsonAdGroupAd> = HashMap::new();
    for a in &live.ad_group_ads {
        live_ads_by_ag.entry(a.ad_group.clone()).or_insert(a);
    }
    for d in &declared.ad_group_ads {
        let action = match ad_group_match.get(&d.ad_group) {
            Some(parent_id) => match live_ads_by_ag.get(parent_id) {
                Some(l) => action_for_match(l.id.clone(), diff_ad_group_ad(d, l)),
                None => Action::Create,
            },
            None => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "ad_group_ad",
            action,
        });
    }

    // ---- ad_group_criteria (match by ad_group + keyword) -----------------
    let live_ag_criteria: HashMap<(String, String, String), &JsonAdGroupCriterion> = live
        .ad_group_criteria
        .iter()
        .map(|c| {
            (
                (c.ad_group.clone(), c.keyword.text.clone(), c.keyword.match_type.clone()),
                c,
            )
        })
        .collect();
    for d in &declared.ad_group_criteria {
        let action = match ad_group_match.get(&d.ad_group) {
            Some(parent_id) => {
                let key = (
                    parent_id.clone(),
                    d.keyword.text.clone(),
                    d.keyword.match_type.clone(),
                );
                match live_ag_criteria.get(&key) {
                    Some(l) => action_for_match(l.id.clone(), diff_ad_group_criterion(d, l)),
                    None => Action::Create,
                }
            }
            None => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "ad_group_criterion",
            action,
        });
    }

    // ---- campaign_criteria (match by campaign + criterion key) -----------
    let mut live_c_criteria: HashMap<(String, String), &JsonCampaignCriterion> = HashMap::new();
    for c in &live.campaign_criteria {
        if let Some(key) = campaign_criterion_key(c) {
            live_c_criteria.insert((c.campaign.clone(), key), c);
        }
    }
    for d in &declared.campaign_criteria {
        let action = match (campaign_match.get(&d.campaign), campaign_criterion_key(d)) {
            (Some(parent_id), Some(key)) => match live_c_criteria.get(&(parent_id.clone(), key)) {
                Some(l) => action_for_match(l.id.clone(), diff_campaign_criterion(d, l)),
                None => Action::Create,
            },
            _ => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "campaign_criterion",
            action,
        });
    }

    let live_conversion_actions: HashMap<&str, &JsonConversionAction> = live
        .conversion_actions
        .iter()
        .map(|c| (c.name.as_str(), c))
        .collect();
    for d in &declared.conversion_actions {
        let action = match live_conversion_actions.get(d.name.as_str()) {
            Some(l) => {
                conversion_match.insert(d.id.clone(), l.id.clone());
                action_for_match(l.id.clone(), diff_conversion_action(d, l))
            }
            None => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "conversion_action",
            action,
        });
    }

    let live_call_assets: HashMap<(String, String), &JsonCallAsset> = live
        .call_assets
        .iter()
        .map(|a| ((a.country_code.clone(), a.phone_number.clone()), a))
        .collect();
    for d in &declared.call_assets {
        let action = match live_call_assets
            .get(&(d.country_code.clone(), d.phone_number.clone()))
        {
            Some(l) => {
                call_asset_match.insert(d.id.clone(), l.id.clone());
                action_for_match(l.id.clone(), diff_call_asset(d, l))
            }
            None => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "call_asset",
            action,
        });
    }

    let mut live_customer_assets: HashMap<(String, String), &JsonCustomerAsset> = HashMap::new();
    for a in &live.customer_assets {
        live_customer_assets.insert((a.asset.clone(), a.field_type.clone()), a);
    }
    for d in &declared.customer_assets {
        let action = match call_asset_match.get(&d.asset) {
            Some(asset_id) => match live_customer_assets
                .get(&(asset_id.clone(), d.field_type.clone()))
            {
                Some(l) => action_for_match(l.id.clone(), diff_customer_asset(d, l)),
                None => Action::Create,
            },
            None => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "customer_asset",
            action,
        });
    }

    let _ = conversion_match;

    let mut shared_set_match: HashMap<String, String> = HashMap::new();
    let live_shared_sets: HashMap<&str, &JsonSharedSet> = live
        .shared_sets
        .iter()
        .map(|s| (s.name.as_str(), s))
        .collect();
    for d in &declared.shared_sets {
        let action = match live_shared_sets.get(d.name.as_str()) {
            Some(l) => {
                shared_set_match.insert(d.id.clone(), l.id.clone());
                action_for_match(l.id.clone(), diff_shared_set(d, l))
            }
            None => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "shared_set",
            action,
        });
    }

    let live_shared_criteria: HashMap<(String, String, String), &JsonSharedCriterion> = live
        .shared_criteria
        .iter()
        .map(|c| {
            (
                (
                    c.shared_set.clone(),
                    c.keyword.match_type.clone(),
                    c.keyword.text.clone(),
                ),
                c,
            )
        })
        .collect();
    for d in &declared.shared_criteria {
        let live_set_id = shared_set_match.get(&d.shared_set).cloned().or_else(|| {
            if d.shared_set.starts_with("customers/") {
                d.shared_set.rsplit('/').next().map(str::to_string)
            } else {
                None
            }
        });
        let action = match live_set_id {
            Some(set_id) => match live_shared_criteria.get(&(
                set_id,
                d.keyword.match_type.clone(),
                d.keyword.text.clone(),
            )) {
                Some(l) => action_for_match(l.id.clone(), Vec::new()),
                None => Action::Create,
            },
            None => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "shared_criterion",
            action,
        });
    }

    let live_campaign_shared_sets: HashMap<(String, String), &JsonCampaignSharedSet> = live
        .campaign_shared_sets
        .iter()
        .map(|s| ((s.campaign.clone(), s.shared_set.clone()), s))
        .collect();
    for d in &declared.campaign_shared_sets {
        let action = match (
            campaign_match.get(&d.campaign),
            shared_set_match.get(&d.shared_set),
        ) {
            (Some(c_id), Some(s_id)) => {
                match live_campaign_shared_sets.get(&(c_id.clone(), s_id.clone())) {
                    Some(l) => action_for_match(l.id.clone(), diff_campaign_shared_set(d, l)),
                    None => Action::Create,
                }
            }
            _ => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "campaign_shared_set",
            action,
        });
    }

    let deletes = orphan_criteria_deletes(
        declared,
        live,
        &diffs,
        &ad_group_match,
        &campaign_match,
        &shared_set_match,
    );
    diffs.extend(deletes);

    let mut noop_count = 0;
    let mut create_count = 0;
    let mut update_count = 0;
    let mut delete_count = 0;
    for d in &diffs {
        match &d.action {
            Action::NoOp { .. } => noop_count += 1,
            Action::Create => create_count += 1,
            Action::Update { .. } => update_count += 1,
            Action::Delete { .. } => delete_count += 1,
        }
    }

    DiffReport {
        diffs,
        noop_count,
        create_count,
        update_count,
        delete_count,
    }
}

/// A live criterion stops being declared in two ways: its whole parent
/// resource is dropped, or — the silent case — a `negative_keyword` /
/// `keyword` block is deleted from a resource that otherwise survives. The
/// first needs `bidsmith:address` labels to prune safely; the second doesn't,
/// because the parent is still declared and the declared members are the
/// authoritative set *within their category*. We prune only inside a
/// `(parent, category)` the file already claims (has ≥1 declared member of),
/// so declaring negatives never deletes positives a user manages elsewhere.
fn orphan_criteria_deletes(
    declared: &ExportInput,
    live: &ExportInput,
    diffs: &[ResourceDiff],
    ad_group_match: &HashMap<String, String>,
    campaign_match: &HashMap<String, String>,
    shared_set_match: &HashMap<String, String>,
) -> Vec<ResourceDiff> {
    let mut out: Vec<ResourceDiff> = Vec::new();

    let matched_live_ids = |kind: &str| -> std::collections::HashSet<&str> {
        diffs
            .iter()
            .filter(|d| d.kind == kind)
            .filter_map(|d| d.action.live_id())
            .collect()
    };
    let reverse = |m: &HashMap<String, String>| -> HashMap<String, String> {
        m.iter().map(|(addr, id)| (id.clone(), addr.clone())).collect()
    };

    // ---- ad_group_criteria: category = negative polarity ----------------
    {
        let matched = matched_live_ids("ad_group_criterion");
        let parent_addr = reverse(ad_group_match);
        let mut managed: std::collections::HashSet<(String, bool)> = std::collections::HashSet::new();
        for d in &declared.ad_group_criteria {
            if let Some(live_ag) = ad_group_match.get(&d.ad_group) {
                managed.insert((live_ag.clone(), d.negative.unwrap_or(false)));
            }
        }
        for l in &live.ad_group_criteria {
            if matched.contains(l.id.as_str()) {
                continue;
            }
            let polarity = l.negative.unwrap_or(false);
            if !managed.contains(&(l.ad_group.clone(), polarity)) {
                continue;
            }
            let word = if polarity { "negative_keyword" } else { "keyword" };
            let descriptor = format!("{word} \"{}\" {}", l.keyword.text, l.keyword.match_type);
            out.push(delete_diff(
                "ad_group_criterion",
                parent_addr.get(&l.ad_group),
                &l.ad_group,
                "adGroups",
                &descriptor,
                &l.id,
            ));
        }
    }

    // ---- campaign_criteria: category = kw polarity / location / language / proximity ----
    {
        let matched = matched_live_ids("campaign_criterion");
        let parent_addr = reverse(campaign_match);
        let mut managed: std::collections::HashSet<(String, &'static str)> =
            std::collections::HashSet::new();
        for d in &declared.campaign_criteria {
            if let Some(live_c) = campaign_match.get(&d.campaign) {
                managed.insert((live_c.clone(), campaign_criterion_category(d)));
            }
        }
        for l in &live.campaign_criteria {
            if matched.contains(l.id.as_str()) {
                continue;
            }
            let category = campaign_criterion_category(l);
            if !managed.contains(&(l.campaign.clone(), category)) {
                continue;
            }
            let Some(descriptor) = campaign_criterion_descriptor(l) else {
                continue;
            };
            out.push(delete_diff(
                "campaign_criterion",
                parent_addr.get(&l.campaign),
                &l.campaign,
                "campaigns",
                &descriptor,
                &l.id,
            ));
        }
    }

    // ---- shared_criteria: one category (set membership) -----------------
    {
        let matched = matched_live_ids("shared_criterion");
        let parent_addr = reverse(shared_set_match);
        let resolve_set = |declared_ref: &str| -> Option<String> {
            shared_set_match.get(declared_ref).cloned().or_else(|| {
                declared_ref
                    .starts_with("customers/")
                    .then(|| declared_ref.rsplit('/').next().map(str::to_string))
                    .flatten()
            })
        };
        let mut managed: std::collections::HashSet<String> = std::collections::HashSet::new();
        for d in &declared.shared_criteria {
            if let Some(set_id) = resolve_set(&d.shared_set) {
                managed.insert(set_id);
            }
        }
        for l in &live.shared_criteria {
            if matched.contains(l.id.as_str()) {
                continue;
            }
            if !managed.contains(&l.shared_set) {
                continue;
            }
            let descriptor =
                format!("negative_keyword \"{}\" {}", l.keyword.text, l.keyword.match_type);
            out.push(delete_diff(
                "shared_criterion",
                parent_addr.get(&l.shared_set),
                &l.shared_set,
                "sharedSets",
                &descriptor,
                &l.id,
            ));
        }
    }

    out
}

fn delete_diff(
    kind: &'static str,
    parent_addr: Option<&String>,
    parent_live_id: &str,
    parent_segment: &str,
    descriptor: &str,
    live_id: &str,
) -> ResourceDiff {
    let anchor = parent_addr
        .cloned()
        .unwrap_or_else(|| format!("{parent_segment}/{parent_live_id}"));
    ResourceDiff {
        address: format!("{anchor} (removed {descriptor})"),
        kind,
        action: Action::Delete {
            live_id: live_id.to_string(),
        },
    }
}

fn campaign_criterion_category(cr: &JsonCampaignCriterion) -> &'static str {
    if cr.keyword.is_some() {
        if cr.negative.unwrap_or(false) {
            "keyword_negative"
        } else {
            "keyword_positive"
        }
    } else if cr.location.is_some() {
        "location"
    } else if cr.language.is_some() {
        "language"
    } else if cr.proximity.is_some() {
        "proximity"
    } else {
        "other"
    }
}

fn campaign_criterion_descriptor(cr: &JsonCampaignCriterion) -> Option<String> {
    if let Some(kw) = &cr.keyword {
        let word = if cr.negative.unwrap_or(false) {
            "negative_keyword"
        } else {
            "keyword"
        };
        Some(format!("{word} \"{}\" {}", kw.text, kw.match_type))
    } else if let Some(loc) = &cr.location {
        Some(format!("location {}", loc.geo_target_constant))
    } else if let Some(lang) = &cr.language {
        Some(format!("language {}", lang.language_constant))
    } else {
        cr.proximity.as_ref().map(|_| "proximity".to_string())
    }
}

fn action_for_match(live_id: String, changed: Vec<String>) -> Action {
    if changed.is_empty() {
        Action::NoOp { live_id }
    } else {
        Action::Update {
            live_id,
            changed_fields: changed,
        }
    }
}

fn diff_budget(d: &JsonBudget, l: &JsonBudget) -> Vec<String> {
    let mut c = Vec::new();
    if d.name != l.name {
        c.push("name".into());
    }
    if d.amount_micros != l.amount_micros {
        c.push("amount_micros".into());
    }
    if d.delivery_method != l.delivery_method {
        c.push("delivery_method".into());
    }
    if d.explicitly_shared != l.explicitly_shared {
        c.push("explicitly_shared".into());
    }
    c
}

fn diff_campaign(d: &JsonCampaign, l: &JsonCampaign) -> Vec<String> {
    let mut c = Vec::new();
    if d.name != l.name {
        c.push("name".into());
    }
    if d.status != l.status {
        c.push("status".into());
    }
    if d.contains_eu_political_advertising != l.contains_eu_political_advertising
        && d.contains_eu_political_advertising.is_some()
    {
        c.push("contains_eu_political_advertising".into());
    }
    // advertising_channel_type is creation-only; skip.
    let dm = d.manual_cpc.as_ref().and_then(|m| m.enhanced_cpc_enabled);
    let lm = l.manual_cpc.as_ref().and_then(|m| m.enhanced_cpc_enabled);
    if dm != lm {
        c.push("manual_cpc.enhanced_cpc_enabled".into());
    }
    let pairs = [
        ("network_settings.target_google_search",
            d.network_settings.as_ref().and_then(|n| n.target_google_search),
            l.network_settings.as_ref().and_then(|n| n.target_google_search)),
        ("network_settings.target_search_network",
            d.network_settings.as_ref().and_then(|n| n.target_search_network),
            l.network_settings.as_ref().and_then(|n| n.target_search_network)),
        ("network_settings.target_content_network",
            d.network_settings.as_ref().and_then(|n| n.target_content_network),
            l.network_settings.as_ref().and_then(|n| n.target_content_network)),
        ("network_settings.target_partner_search_network",
            d.network_settings.as_ref().and_then(|n| n.target_partner_search_network),
            l.network_settings.as_ref().and_then(|n| n.target_partner_search_network)),
    ];
    for (path, dv, lv) in pairs {
        if dv != lv {
            c.push(path.into());
        }
    }
    c
}

fn diff_ad_group(d: &JsonAdGroup, l: &JsonAdGroup) -> Vec<String> {
    let mut c = Vec::new();
    if d.name != l.name {
        c.push("name".into());
    }
    if d.status != l.status {
        c.push("status".into());
    }
    if d.ty != l.ty {
        c.push("type".into());
    }
    if d.cpc_bid_micros != l.cpc_bid_micros {
        c.push("cpc_bid_micros".into());
    }
    c
}

fn diff_ad_group_ad(d: &JsonAdGroupAd, l: &JsonAdGroupAd) -> Vec<String> {
    let mut c = Vec::new();
    if d.status != l.status {
        c.push("status".into());
    }
    // ad.* fields are creation-only / a new ad is the way to "edit" copy.
    c
}

fn diff_ad_group_criterion(d: &JsonAdGroupCriterion, l: &JsonAdGroupCriterion) -> Vec<String> {
    let mut c = Vec::new();
    if d.status != l.status {
        c.push("status".into());
    }
    if d.negative != l.negative {
        c.push("negative".into());
    }
    if d.cpc_bid_micros != l.cpc_bid_micros {
        c.push("cpc_bid_micros".into());
    }
    // keyword.text / match_type are creation-only.
    c
}

fn diff_campaign_criterion(d: &JsonCampaignCriterion, l: &JsonCampaignCriterion) -> Vec<String> {
    let mut c = Vec::new();
    if d.status != l.status {
        c.push("status".into());
    }
    if d.negative != l.negative {
        c.push("negative".into());
    }
    c
}

fn diff_conversion_action(d: &JsonConversionAction, l: &JsonConversionAction) -> Vec<String> {
    let mut c = Vec::new();
    if d.status != l.status {
        c.push("status".into());
    }
    if d.counting_type != l.counting_type {
        c.push("counting_type".into());
    }
    if d.click_through_lookback_window_days != l.click_through_lookback_window_days {
        c.push("click_through_lookback_window_days".into());
    }
    if d.view_through_lookback_window_days != l.view_through_lookback_window_days {
        c.push("view_through_lookback_window_days".into());
    }
    let dv = d.value_settings.as_ref().and_then(|v| v.default_value);
    let lv = l.value_settings.as_ref().and_then(|v| v.default_value);
    if dv != lv {
        c.push("value_settings.default_value".into());
    }
    let dc = d
        .value_settings
        .as_ref()
        .and_then(|v| v.default_currency_code.clone());
    let lc = l
        .value_settings
        .as_ref()
        .and_then(|v| v.default_currency_code.clone());
    if dc != lc {
        c.push("value_settings.default_currency_code".into());
    }
    let da = d
        .value_settings
        .as_ref()
        .and_then(|v| v.always_use_default_value);
    let la = l
        .value_settings
        .as_ref()
        .and_then(|v| v.always_use_default_value);
    if da != la {
        c.push("value_settings.always_use_default_value".into());
    }
    c
}

fn diff_call_asset(_d: &JsonCallAsset, _l: &JsonCallAsset) -> Vec<String> {
    Vec::new()
}

fn diff_customer_asset(d: &JsonCustomerAsset, l: &JsonCustomerAsset) -> Vec<String> {
    let mut c = Vec::new();
    if d.status != l.status {
        c.push("status".into());
    }
    c
}

fn diff_shared_set(d: &JsonSharedSet, l: &JsonSharedSet) -> Vec<String> {
    let mut c = Vec::new();
    if d.status != l.status {
        c.push("status".into());
    }
    if d.ty != l.ty && d.ty.is_some() {
        c.push("type".into());
    }
    c
}

fn diff_campaign_shared_set(d: &JsonCampaignSharedSet, l: &JsonCampaignSharedSet) -> Vec<String> {
    let mut c = Vec::new();
    if d.status != l.status {
        c.push("status".into());
    }
    c
}

fn campaign_criterion_key(cr: &JsonCampaignCriterion) -> Option<String> {
    if let Some(kw) = &cr.keyword {
        return Some(format!("kw:{}|{}", kw.match_type, kw.text));
    }
    if let Some(loc) = &cr.location {
        return Some(format!("loc:{}", loc.geo_target_constant));
    }
    if let Some(lang) = &cr.language {
        return Some(format!("lang:{}", lang.language_constant));
    }
    if let Some(p) = &cr.proximity {
        return Some(format!(
            "prox:{}:{}:{:.6}:{}",
            (p.latitude * 1_000_000.0).round() as i64,
            (p.longitude * 1_000_000.0).round() as i64,
            p.radius,
            p.radius_units,
        ));
    }
    None
}
