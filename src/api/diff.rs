use std::collections::HashMap;

use crate::commands::export::{
    ExportInput, JsonAdGroup, JsonAdGroupAd, JsonAdGroupCriterion, JsonBudget, JsonCampaign,
    JsonCampaignCriterion,
};

#[derive(Debug, Clone)]
pub enum Action {
    Existing {
        #[allow(dead_code)]
        live_id: String,
    },
    Create,
}

#[derive(Debug, Clone)]
pub struct ResourceDiff {
    pub address: String,
    #[allow(dead_code)]
    pub kind: &'static str,
    pub action: Action,
}

pub struct DiffReport {
    pub diffs: Vec<ResourceDiff>,
    pub existing_count: usize,
    pub create_count: usize,
}

pub fn diff(declared: &ExportInput, live: &ExportInput) -> DiffReport {
    let mut diffs: Vec<ResourceDiff> = Vec::new();

    // declared address → matched live API id
    let mut campaign_match: HashMap<String, String> = HashMap::new();
    let mut ad_group_match: HashMap<String, String> = HashMap::new();

    // budgets — match by name
    let budget_by_name = index_by_name(&live.campaign_budgets, |b| b.name.as_str(), |b| b.id.clone());
    for b in &declared.campaign_budgets {
        let action = match budget_by_name.get(b.name.as_str()) {
            Some(id) => Action::Existing { live_id: id.clone() },
            None => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: b.id.clone(),
            kind: "campaign_budget",
            action,
        });
    }

    // campaigns — match by name; campaign budget reference is also matched but doesn't gate
    let campaign_by_name = index_by_name(&live.campaigns, |c| c.name.as_str(), |c| c.id.clone());
    for c in &declared.campaigns {
        let action = match campaign_by_name.get(c.name.as_str()) {
            Some(id) => {
                campaign_match.insert(c.id.clone(), id.clone());
                Action::Existing { live_id: id.clone() }
            }
            None => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: c.id.clone(),
            kind: "campaign",
            action,
        });
    }

    // ad_groups — match by (mapped_campaign_id, name)
    let ad_group_by_parent_name: HashMap<(String, String), String> = live
        .ad_groups
        .iter()
        .map(|g| ((g.campaign.clone(), g.name.clone()), g.id.clone()))
        .collect();
    for g in &declared.ad_groups {
        let action = match campaign_match.get(&g.campaign) {
            Some(parent_id) => match ad_group_by_parent_name
                .get(&(parent_id.clone(), g.name.clone()))
            {
                Some(id) => {
                    ad_group_match.insert(g.id.clone(), id.clone());
                    Action::Existing { live_id: id.clone() }
                }
                None => Action::Create,
            },
            None => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: g.id.clone(),
            kind: "ad_group",
            action,
        });
    }

    // ad_group_ads — match any ad in the matched ad_group (rezolutnie pattern is one RSA per ad_group)
    let mut ad_by_ad_group: HashMap<String, Vec<&JsonAdGroupAd>> = HashMap::new();
    for a in &live.ad_group_ads {
        ad_by_ad_group.entry(a.ad_group.clone()).or_default().push(a);
    }
    for a in &declared.ad_group_ads {
        let action = match ad_group_match.get(&a.ad_group) {
            Some(parent_id) => match ad_by_ad_group.get(parent_id).and_then(|v| v.first()) {
                Some(live_ad) => Action::Existing { live_id: live_ad.id.clone() },
                None => Action::Create,
            },
            None => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: a.id.clone(),
            kind: "ad_group_ad",
            action,
        });
    }

    // ad_group_criteria — match by (mapped_ad_group_id, keyword.text, keyword.match_type)
    let mut crit_by_key: HashMap<(String, String, String), String> = HashMap::new();
    for cr in &live.ad_group_criteria {
        let key = (cr.ad_group.clone(), cr.keyword.text.clone(), cr.keyword.match_type.clone());
        crit_by_key.insert(key, cr.id.clone());
    }
    for cr in &declared.ad_group_criteria {
        let action = match ad_group_match.get(&cr.ad_group) {
            Some(parent_id) => {
                let key = (parent_id.clone(), cr.keyword.text.clone(), cr.keyword.match_type.clone());
                match crit_by_key.get(&key) {
                    Some(id) => Action::Existing { live_id: id.clone() },
                    None => Action::Create,
                }
            }
            None => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: cr.id.clone(),
            kind: "ad_group_criterion",
            action,
        });
    }

    // campaign_criteria — match by (mapped_campaign_id, criterion_kind, criterion_key)
    let mut camp_crit_by_key: HashMap<(String, String), String> = HashMap::new();
    for cr in &live.campaign_criteria {
        if let Some(key) = campaign_criterion_key(cr) {
            camp_crit_by_key.insert((cr.campaign.clone(), key), cr.id.clone());
        }
    }
    for cr in &declared.campaign_criteria {
        let action = match (campaign_match.get(&cr.campaign), campaign_criterion_key(cr)) {
            (Some(parent_id), Some(key)) => {
                match camp_crit_by_key.get(&(parent_id.clone(), key)) {
                    Some(id) => Action::Existing { live_id: id.clone() },
                    None => Action::Create,
                }
            }
            _ => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: cr.id.clone(),
            kind: "campaign_criterion",
            action,
        });
    }

    let existing_count = diffs
        .iter()
        .filter(|d| matches!(d.action, Action::Existing { .. }))
        .count();
    let create_count = diffs.len() - existing_count;
    DiffReport {
        diffs,
        existing_count,
        create_count,
    }
}

fn index_by_name<T, F, G>(items: &[T], name_of: F, id_of: G) -> HashMap<String, String>
where
    F: Fn(&T) -> &str,
    G: Fn(&T) -> String,
{
    let mut m: HashMap<String, String> = HashMap::new();
    for item in items {
        m.insert(name_of(item).to_string(), id_of(item));
    }
    m
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
            p.geo_point.latitude_in_micro_degrees,
            p.geo_point.longitude_in_micro_degrees,
            p.radius,
            p.radius_units,
        ));
    }
    None
}

// Suppress unused-warning until we wire updates in 4d.
#[allow(dead_code)]
fn _suppress(
    _: &JsonBudget,
    _: &JsonCampaign,
    _: &JsonAdGroup,
    _: &JsonAdGroupAd,
    _: &JsonAdGroupCriterion,
) {
}
