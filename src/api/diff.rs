use std::collections::HashMap;

use crate::commands::export::{
    ExportInput, JsonAdGroup, JsonAdGroupAd, JsonAdGroupCriterion, JsonBudget, JsonCampaign,
    JsonCampaignCriterion,
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
}

impl Action {
    pub fn live_id(&self) -> Option<&str> {
        match self {
            Action::NoOp { live_id } | Action::Update { live_id, .. } => Some(live_id.as_str()),
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
}

pub fn diff(declared: &ExportInput, live: &ExportInput) -> DiffReport {
    let mut diffs: Vec<ResourceDiff> = Vec::new();
    let mut campaign_match: HashMap<String, String> = HashMap::new();
    let mut ad_group_match: HashMap<String, String> = HashMap::new();

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

    let mut noop_count = 0;
    let mut create_count = 0;
    let mut update_count = 0;
    for d in &diffs {
        match &d.action {
            Action::NoOp { .. } => noop_count += 1,
            Action::Create => create_count += 1,
            Action::Update { .. } => update_count += 1,
        }
    }

    DiffReport {
        diffs,
        noop_count,
        create_count,
        update_count,
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
