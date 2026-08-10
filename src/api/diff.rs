use std::collections::HashMap;

use crate::commands::export::{
    address_label_payload, ExportInput, JsonAdGroup, JsonAdGroupAd, JsonAdGroupAsset,
    JsonAdGroupCriterion, JsonBudget, JsonCallAsset, JsonCalloutAsset, JsonCampaign,
    JsonCampaignAsset, JsonCampaignCriterion, JsonCampaignSharedSet, JsonConversionAction,
    JsonAudience, JsonCustomAudience, JsonCustomerAsset, JsonSharedCriterion, JsonSharedSet,
    JsonSitelinkAsset, JsonStructuredSnippetAsset, JsonYoutubeVideoAsset,
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

/// A `bidsmith:address` label to write (or reconcile) on a resource as part of
/// applying the diff. Carried alongside the per-resource actions because the
/// label is bidsmith's identity key for the labelable kinds (campaign,
/// ad_group, ad_group_ad).
#[derive(Debug, Clone)]
pub struct LabelPlanEntry {
    /// Address of the resource the label attaches to (key into the mutate
    /// builder's resource_name map).
    pub address: String,
    /// `campaign` | `ad_group` | `ad_group_ad`.
    pub kind: &'static str,
    /// The `bidsmith:address=<this>` value the resource should carry.
    pub label_address: String,
    /// An existing live label resource_name to reuse; None -> create one.
    pub existing_label_rn: Option<String>,
    /// A stale bidsmith label association to remove (relabel on a surviving
    /// resource). None when the resource is new or already correctly labeled.
    pub stale_assoc_rn: Option<String>,
}

/// A `bidsmith:owns=<category>` label association to add to (or remove from) a
/// campaign / ad group. The claim records that bidsmith manages that criterion
/// category on the parent, so orphaned members still plan as destroys after the
/// last declared member of the category is removed — criteria themselves can't
/// carry identity labels (the API forbids labels on negative criteria).
#[derive(Debug, Clone)]
pub struct ClaimPlanEntry {
    /// Declared address of the parent the claim attaches to.
    pub address: String,
    /// `campaign` | `ad_group`.
    pub kind: &'static str,
    /// Criterion category token (`keyword_negative`, `location`, ...).
    pub category: &'static str,
    /// An existing live `bidsmith:owns` label resource_name to reuse; None ->
    /// create one. Only meaningful for claim additions.
    pub existing_label_rn: Option<String>,
    /// A live claim association to remove — set when the category is no longer
    /// declared on this parent (claim release). None for claim additions.
    pub stale_assoc_rn: Option<String>,
}

pub struct DiffReport {
    pub diffs: Vec<ResourceDiff>,
    pub label_plans: Vec<LabelPlanEntry>,
    pub claim_plans: Vec<ClaimPlanEntry>,
    pub noop_count: usize,
    pub create_count: usize,
    pub update_count: usize,
    pub delete_count: usize,
    /// Resources that match live with no field change but still need label
    /// work — a `bidsmith:address` write (first-run adoption) or a
    /// `bidsmith:owns` claim add / release. Counted so plan / apply treat
    /// label-only work as a pending change rather than a no-op.
    pub adopt_count: usize,
    /// Live drift bidsmith cannot reconcile (e.g. an undeclared device
    /// modifier — the API forbids removing device criteria). Printed by
    /// plan / apply; never turned into mutate ops.
    pub warnings: Vec<String>,
}

/// Label-first match with content fallback over parallel declared / live
/// arrays. A declared resource matches the live one carrying its
/// `bidsmith:address` label; failing that, it falls back to a content key
/// (name / parent+name) to adopt an unlabeled live resource. Returns the
/// matched live index per declared item (None = create); each live index is
/// claimed at most once. The caller derives the claimed set (the `Some`
/// values) for whole-resource removal detection.
fn match_label_first(
    declared_addr: &[&str],
    declared_key: &[Option<String>],
    live_addr: &[Option<&str>],
    live_key: &[String],
) -> Vec<Option<usize>> {
    let mut claimed = vec![false; live_addr.len()];
    let mut result: Vec<Option<usize>> = vec![None; declared_addr.len()];

    let mut by_addr: HashMap<&str, usize> = HashMap::new();
    for (i, a) in live_addr.iter().enumerate() {
        if let Some(a) = a {
            by_addr.entry(a).or_insert(i);
        }
    }
    for (di, addr) in declared_addr.iter().enumerate() {
        if let Some(&li) = by_addr.get(addr) {
            if !claimed[li] {
                claimed[li] = true;
                result[di] = Some(li);
            }
        }
    }

    let mut by_key: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, k) in live_key.iter().enumerate() {
        if !claimed[i] {
            by_key.entry(k.as_str()).or_default().push(i);
        }
    }
    for (di, key) in declared_key.iter().enumerate() {
        if result[di].is_some() {
            continue;
        }
        if let Some(key) = key {
            if let Some(cands) = by_key.get(key.as_str()) {
                if let Some(&li) = cands.iter().find(|&&li| !claimed[li]) {
                    claimed[li] = true;
                    result[di] = Some(li);
                }
            }
        }
    }
    result
}

fn label_segment(kind: &str) -> &'static str {
    match kind {
        "campaign" => "campaignLabels",
        "ad_group" => "adGroupLabels",
        "ad_group_ad" => "adGroupAdLabels",
        "ad_group_criterion" => "adGroupCriterionLabels",
        _ => "labels",
    }
}

/// Decide what label work a labelable resource needs. `matched` is the live
/// resource it resolved to (id + its current `bidsmith:address` payload), or
/// None for a create. Returns None when the resource already carries exactly the
/// right label (a no-op). All comparisons run in label-payload space so long
/// addresses (whose labels are hash-encoded to fit the 80-char cap) still match.
fn make_label_plan(
    kind: &'static str,
    address: &str,
    matched: Option<(&str, Option<&str>)>,
    customer_id: &str,
    labels: &HashMap<String, String>,
) -> Option<LabelPlanEntry> {
    let payload = address_label_payload(address);
    if let Some((_, Some(current))) = matched {
        if current == payload {
            return None;
        }
    }
    let stale_assoc_rn = match matched {
        Some((live_id, Some(old))) if old != payload => labels.get(old).map(|label_rn| {
            let label_id = label_rn.rsplit('/').next().unwrap_or(label_rn);
            format!(
                "customers/{customer_id}/{}/{live_id}~{label_id}",
                label_segment(kind)
            )
        }),
        _ => None,
    };
    Some(LabelPlanEntry {
        address: address.to_string(),
        kind,
        existing_label_rn: labels.get(&payload).cloned(),
        label_address: payload,
        stale_assoc_rn,
    })
}

pub fn diff(declared: &ExportInput, live: &ExportInput) -> DiffReport {
    let mut diffs: Vec<ResourceDiff> = Vec::new();
    let mut label_plans: Vec<LabelPlanEntry> = Vec::new();
    let mut campaign_match: HashMap<String, String> = HashMap::new();
    let mut ad_group_match: HashMap<String, String> = HashMap::new();
    // declared asset address -> live asset id, across every asset type
    // (call / sitelink / callout / structured snippet / youtube video).
    // Consumed by the customer_asset / campaign_asset / ad_group_asset link
    // diffs and by the video ad body key.
    let mut asset_match: HashMap<String, String> = HashMap::new();
    let customer_id = declared.customer_id.as_str();

    // Video assets resolve ahead of the ad_group_ad pass: a video ad's body key
    // names its video by live asset id, so the mapping has to exist before ads
    // are matched. The diff entries themselves are pushed with the other assets.
    let live_youtube_video_assets: HashMap<&str, &JsonYoutubeVideoAsset> = live
        .youtube_video_assets
        .iter()
        .map(|a| (a.youtube_video_id.as_str(), a))
        .collect();
    for d in &declared.youtube_video_assets {
        if let Some(l) = live_youtube_video_assets.get(d.youtube_video_id.as_str()) {
            asset_match.insert(d.id.clone(), l.id.clone());
        }
    }

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

    // ---- campaigns (label-first, content-fallback by name) ----------------
    let live_campaigns: Vec<&JsonCampaign> = live.campaigns.iter().collect();
    {
        let decl_payloads: Vec<String> =
            declared.campaigns.iter().map(|c| address_label_payload(&c.id)).collect();
        let decl_addr: Vec<&str> = decl_payloads.iter().map(String::as_str).collect();
        let decl_key: Vec<Option<String>> =
            declared.campaigns.iter().map(|c| Some(c.name.clone())).collect();
        let live_a: Vec<Option<&str>> =
            live_campaigns.iter().map(|c| c.managed_address.as_deref()).collect();
        let live_k: Vec<String> = live_campaigns.iter().map(|c| c.name.clone()).collect();
        let matches = match_label_first(&decl_addr, &decl_key, &live_a, &live_k);
        let mut claimed = vec![false; live_campaigns.len()];
        for (di, m) in matches.iter().enumerate() {
            let d = &declared.campaigns[di];
            let (action, matched) = match m {
                Some(li) => {
                    let l = live_campaigns[*li];
                    claimed[*li] = true;
                    campaign_match.insert(d.id.clone(), l.id.clone());
                    (
                        action_for_match(l.id.clone(), diff_campaign(d, l)),
                        Some((l.id.as_str(), l.managed_address.as_deref())),
                    )
                }
                None => (Action::Create, None),
            };
            if let Some(plan) =
                make_label_plan("campaign", &d.id, matched, customer_id, &live.labels)
            {
                label_plans.push(plan);
            }
            diffs.push(ResourceDiff {
                address: d.id.clone(),
                kind: "campaign",
                action,
            });
        }
        for (li, l) in live_campaigns.iter().enumerate() {
            if !claimed[li] && !is_removed(l.status.as_deref()) {
                if let Some(addr) = &l.managed_address {
                    diffs.push(removal_diff("campaign", addr, &l.id));
                }
            }
        }
    }

    // ---- ad_groups (label-first, content-fallback by campaign + name) -----
    let live_ad_groups: Vec<&JsonAdGroup> = live.ad_groups.iter().collect();
    {
        let decl_payloads: Vec<String> =
            declared.ad_groups.iter().map(|g| address_label_payload(&g.id)).collect();
        let decl_addr: Vec<&str> = decl_payloads.iter().map(String::as_str).collect();
        let decl_key: Vec<Option<String>> = declared
            .ad_groups
            .iter()
            .map(|g| campaign_match.get(&g.campaign).map(|cid| format!("{cid}\u{1f}{}", g.name)))
            .collect();
        let live_a: Vec<Option<&str>> =
            live_ad_groups.iter().map(|g| g.managed_address.as_deref()).collect();
        let live_k: Vec<String> = live_ad_groups
            .iter()
            .map(|g| format!("{}\u{1f}{}", g.campaign, g.name))
            .collect();
        let matches = match_label_first(&decl_addr, &decl_key, &live_a, &live_k);
        let mut claimed = vec![false; live_ad_groups.len()];
        for (di, m) in matches.iter().enumerate() {
            let d = &declared.ad_groups[di];
            let (action, matched) = match m {
                Some(li) => {
                    let l = live_ad_groups[*li];
                    claimed[*li] = true;
                    ad_group_match.insert(d.id.clone(), l.id.clone());
                    (
                        action_for_match(l.id.clone(), diff_ad_group(d, l)),
                        Some((l.id.as_str(), l.managed_address.as_deref())),
                    )
                }
                None => (Action::Create, None),
            };
            if let Some(plan) =
                make_label_plan("ad_group", &d.id, matched, customer_id, &live.labels)
            {
                label_plans.push(plan);
            }
            diffs.push(ResourceDiff {
                address: d.id.clone(),
                kind: "ad_group",
                action,
            });
        }
        for (li, l) in live_ad_groups.iter().enumerate() {
            if !claimed[li] && !is_removed(l.status.as_deref()) {
                if let Some(addr) = &l.managed_address {
                    diffs.push(removal_diff("ad_group", addr, &l.id));
                }
            }
        }
    }

    // ---- ad_group_ads (label hit, else body 1:1; label authorizes destroy) -
    let ad_outcomes = match_ad_group_ads(
        &declared.ad_group_ads,
        &live.ad_group_ads,
        &ad_group_match,
        &asset_match,
    );
    let mut ad_claimed = vec![false; live.ad_group_ads.len()];
    for (di, (action, li)) in ad_outcomes.iter().enumerate() {
        let d = &declared.ad_group_ads[di];
        let matched = match li {
            Some(i) => {
                ad_claimed[*i] = true;
                let l = &live.ad_group_ads[*i];
                Some((l.id.as_str(), l.managed_address.as_deref()))
            }
            None => None,
        };
        if let Some(plan) =
            make_label_plan("ad_group_ad", &d.id, matched, customer_id, &live.labels)
        {
            label_plans.push(plan);
        }
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "ad_group_ad",
            action: action.clone(),
        });
    }
    let live_ad_group_ids: std::collections::HashSet<&str> =
        live.ad_groups.iter().map(|g| g.id.as_str()).collect();
    for (i, l) in live.ad_group_ads.iter().enumerate() {
        if ad_claimed[i] {
            continue;
        }
        // Removing an ad group leaves its ads addressable but un-mutatable: the
        // ad keeps its `bidsmith:address` label yet the API rejects any op on it
        // ("Removed ads may not be modified"), sinking the whole atomic batch. An
        // ad already `REMOVED`, or orphaned under an ad group that is gone from
        // live state (i.e. removed), must not re-plan as a destroy.
        if is_removed(l.status.as_deref()) || !live_ad_group_ids.contains(l.ad_group.as_str()) {
            continue;
        }
        if let Some(addr) = &l.managed_address {
            diffs.push(removal_diff("ad_group_ad", addr, &l.id));
        }
    }

    // ---- ad_group_criteria (match by ad_group + keyword) -----------------
    let live_ag_criteria: HashMap<(String, bool, String, String), &JsonAdGroupCriterion> = live
        .ad_group_criteria
        .iter()
        .map(|c| {
            (
                (
                    c.ad_group.clone(),
                    c.negative.unwrap_or(false),
                    c.keyword.text.clone(),
                    c.keyword.match_type.clone(),
                ),
                c,
            )
        })
        .collect();
    for d in &declared.ad_group_criteria {
        let action = match ad_group_match.get(&d.ad_group) {
            Some(parent_id) => {
                let key = (
                    parent_id.clone(),
                    d.negative.unwrap_or(false),
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

    // Ahead of the criteria that reference them: an `audience` criterion is
    // keyed on the live custom audience id, which this match supplies.
    let mut custom_audience_match: HashMap<String, String> = HashMap::new();
    let live_custom_audiences: HashMap<&str, &JsonCustomAudience> = live
        .custom_audiences
        .iter()
        .map(|a| (a.name.as_str(), a))
        .collect();
    for d in &declared.custom_audiences {
        let action = match live_custom_audiences.get(d.name.as_str()) {
            Some(l) => {
                custom_audience_match.insert(d.id.clone(), l.id.clone());
                action_for_match(l.id.clone(), diff_custom_audience(d, l))
            }
            None => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "custom_audience",
            action,
        });
    }

    // ---- campaign_criteria (match by campaign + criterion key) -----------
    let mut live_c_criteria: HashMap<(String, String), &JsonCampaignCriterion> = HashMap::new();
    for c in &live.campaign_criteria {
        if let Some(key) = campaign_criterion_key(c, &HashMap::new()) {
            live_c_criteria.insert((c.campaign.clone(), key), c);
        }
    }
    for d in &declared.campaign_criteria {
        let action = match (
            campaign_match.get(&d.campaign),
            campaign_criterion_key(d, &custom_audience_match),
        ) {
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
            Some(l) => action_for_match(l.id.clone(), diff_conversion_action(d, l)),
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
                asset_match.insert(d.id.clone(), l.id.clone());
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

    let live_sitelink_assets: HashMap<String, &JsonSitelinkAsset> = live
        .sitelink_assets
        .iter()
        .map(|a| (sitelink_asset_key(a), a))
        .collect();
    for d in &declared.sitelink_assets {
        let action = match live_sitelink_assets.get(&sitelink_asset_key(d)) {
            Some(l) => {
                asset_match.insert(d.id.clone(), l.id.clone());
                action_for_match(l.id.clone(), Vec::new())
            }
            None => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "sitelink_asset",
            action,
        });
    }

    let live_callout_assets: HashMap<&str, &JsonCalloutAsset> =
        live.callout_assets.iter().map(|a| (a.text.as_str(), a)).collect();
    for d in &declared.callout_assets {
        let action = match live_callout_assets.get(d.text.as_str()) {
            Some(l) => {
                asset_match.insert(d.id.clone(), l.id.clone());
                action_for_match(l.id.clone(), Vec::new())
            }
            None => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "callout_asset",
            action,
        });
    }

    let live_structured_snippet_assets: HashMap<String, &JsonStructuredSnippetAsset> = live
        .structured_snippet_assets
        .iter()
        .map(|a| (structured_snippet_asset_key(a), a))
        .collect();
    for d in &declared.structured_snippet_assets {
        let action = match live_structured_snippet_assets.get(&structured_snippet_asset_key(d)) {
            Some(l) => {
                asset_match.insert(d.id.clone(), l.id.clone());
                action_for_match(l.id.clone(), Vec::new())
            }
            None => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "structured_snippet_asset",
            action,
        });
    }

    for d in &declared.youtube_video_assets {
        let action = match live_youtube_video_assets.get(d.youtube_video_id.as_str()) {
            Some(l) => action_for_match(l.id.clone(), Vec::new()),
            None => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "youtube_video_asset",
            action,
        });
    }

    let mut live_customer_assets: HashMap<(String, String), &JsonCustomerAsset> = HashMap::new();
    for a in &live.customer_assets {
        live_customer_assets.insert((a.asset.clone(), a.field_type.clone()), a);
    }
    for d in &declared.customer_assets {
        let action = match asset_match.get(&d.asset) {
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

    let live_campaign_assets: HashMap<(String, String, String), &JsonCampaignAsset> = live
        .campaign_assets
        .iter()
        .map(|a| ((a.campaign.clone(), a.asset.clone(), a.field_type.clone()), a))
        .collect();
    for d in &declared.campaign_assets {
        let action = match (resolve_campaign_id(&campaign_match, &d.campaign), asset_match.get(&d.asset)) {
            (Some(campaign_id), Some(asset_id)) => match live_campaign_assets.get(&(
                campaign_id,
                asset_id.clone(),
                d.field_type.clone(),
            )) {
                Some(l) => action_for_match(l.id.clone(), diff_campaign_asset(d, l)),
                None => Action::Create,
            },
            _ => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "campaign_asset",
            action,
        });
    }

    let live_ad_group_assets: HashMap<(String, String, String), &JsonAdGroupAsset> = live
        .ad_group_assets
        .iter()
        .map(|a| ((a.ad_group.clone(), a.asset.clone(), a.field_type.clone()), a))
        .collect();
    for d in &declared.ad_group_assets {
        let action = match (resolve_ad_group_id(&ad_group_match, &d.ad_group), asset_match.get(&d.asset)) {
            (Some(ad_group_id), Some(asset_id)) => match live_ad_group_assets.get(&(
                ad_group_id,
                asset_id.clone(),
                d.field_type.clone(),
            )) {
                Some(l) => action_for_match(l.id.clone(), diff_ad_group_asset(d, l)),
                None => Action::Create,
            },
            _ => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "ad_group_asset",
            action,
        });
    }


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

    let claim_plans = claim_plan_entries(declared, live, &campaign_match, &ad_group_match);

    let (deletes, warnings) = orphan_criteria_deletes(
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

    let noop_addrs: std::collections::HashSet<&str> = diffs
        .iter()
        .filter(|d| matches!(d.action, Action::NoOp { .. }))
        .map(|d| d.address.as_str())
        .collect();
    let adopt_addrs: std::collections::HashSet<&str> = label_plans
        .iter()
        .map(|p| p.address.as_str())
        .chain(claim_plans.iter().map(|p| p.address.as_str()))
        .filter(|a| noop_addrs.contains(a))
        .collect();
    let adopt_count = adopt_addrs.len();

    DiffReport {
        diffs,
        label_plans,
        claim_plans,
        noop_count,
        create_count,
        update_count,
        delete_count,
        adopt_count,
        warnings,
    }
}

/// The criterion categories a `bidsmith:owns` claim can cover. Device is
/// excluded (the API forbids removing device criteria, so a claim would never
/// drive a destroy).
fn canonical_category(cat: &str) -> Option<&'static str> {
    Some(match cat {
        "keyword_positive" => "keyword_positive",
        "keyword_negative" => "keyword_negative",
        "location" => "location",
        "language" => "language",
        "proximity" => "proximity",
        "youtube_channel" => "youtube_channel",
        "youtube_video" => "youtube_video",
        "topic" => "topic",
        "user_interest" => "user_interest",
        "age_range" => "age_range",
        "gender" => "gender",
        "audience" => "audience",
        _ => return None,
    })
}

fn polarity_category(negative: bool) -> &'static str {
    if negative {
        "keyword_negative"
    } else {
        "keyword_positive"
    }
}

/// Reconcile desired category claims (derived from declared criteria) against
/// the live `bidsmith:owns` associations: a desired claim missing live plans an
/// association add; a live claim on a still-declared parent whose category has
/// no declared members plans an association remove. Parents that are no longer
/// declared need nothing — their associations die with the resource.
fn claim_plan_entries(
    declared: &ExportInput,
    live: &ExportInput,
    campaign_match: &HashMap<String, String>,
    ad_group_match: &HashMap<String, String>,
) -> Vec<ClaimPlanEntry> {
    let customer_id = declared.customer_id.as_str();
    let mut out: Vec<ClaimPlanEntry> = Vec::new();

    let mut desired_ag: std::collections::BTreeSet<(&str, &'static str)> =
        std::collections::BTreeSet::new();
    let declared_ags: std::collections::HashSet<&str> =
        declared.ad_groups.iter().map(|g| g.id.as_str()).collect();
    for d in &declared.ad_group_criteria {
        if declared_ags.contains(d.ad_group.as_str()) {
            desired_ag.insert((&d.ad_group, polarity_category(d.negative.unwrap_or(false))));
        }
    }

    let mut desired_c: std::collections::BTreeSet<(&str, &'static str)> =
        std::collections::BTreeSet::new();
    let declared_cs: std::collections::HashSet<&str> =
        declared.campaigns.iter().map(|c| c.id.as_str()).collect();
    for d in &declared.campaign_criteria {
        if declared_cs.contains(d.campaign.as_str()) {
            if let Some(cat) = canonical_category(campaign_criterion_category(d)) {
                desired_c.insert((&d.campaign, cat));
            }
        }
    }

    let mut reconcile = |desired: &std::collections::BTreeSet<(&str, &'static str)>,
                         matches: &HashMap<String, String>,
                         claims: &HashMap<String, Vec<String>>,
                         kind: &'static str| {
        let assoc_segment = label_segment(kind);
        for (addr, cat) in desired {
            let live_has = matches
                .get(*addr)
                .and_then(|id| claims.get(id))
                .is_some_and(|cats| cats.iter().any(|c| c == cat));
            if !live_has {
                out.push(ClaimPlanEntry {
                    address: (*addr).to_string(),
                    kind,
                    category: cat,
                    existing_label_rn: live.claim_labels.get(*cat).cloned(),
                    stale_assoc_rn: None,
                });
            }
        }
        let mut stale: Vec<(&String, &String, &'static str)> = Vec::new();
        for (addr, live_id) in matches {
            let Some(cats) = claims.get(live_id) else {
                continue;
            };
            for cat in cats {
                let Some(tok) = canonical_category(cat) else {
                    continue;
                };
                if !desired.contains(&(addr.as_str(), tok)) {
                    stale.push((addr, live_id, tok));
                }
            }
        }
        stale.sort();
        for (addr, live_id, tok) in stale {
            let Some(label_rn) = live.claim_labels.get(tok) else {
                continue;
            };
            let label_id = label_rn.rsplit('/').next().unwrap_or(label_rn);
            out.push(ClaimPlanEntry {
                address: addr.clone(),
                kind,
                category: tok,
                existing_label_rn: None,
                stale_assoc_rn: Some(format!(
                    "customers/{customer_id}/{assoc_segment}/{live_id}~{label_id}"
                )),
            });
        }
    };

    reconcile(&desired_ag, ad_group_match, &live.ad_group_claims, "ad_group");
    reconcile(&desired_c, campaign_match, &live.campaign_claims, "campaign");
    out
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
) -> (Vec<ResourceDiff>, Vec<String>) {
    let mut out: Vec<ResourceDiff> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

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
        for (live_id, cats) in &live.ad_group_claims {
            if !parent_addr.contains_key(live_id) {
                continue;
            }
            for cat in cats {
                if let Some(negative) = [true, false].into_iter().find(|&n| cat == polarity_category(n)) {
                    managed.insert((live_id.clone(), negative));
                }
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
        for (live_id, cats) in &live.campaign_claims {
            if !parent_addr.contains_key(live_id) {
                continue;
            }
            for cat in cats {
                if let Some(tok) = canonical_category(cat) {
                    managed.insert((live_id.clone(), tok));
                }
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
            // Device criteria can never be removed via the API, and Google
            // auto-materializes every device type once one exists — a remove
            // op is guaranteed to reject and sinks the whole atomic batch.
            if category == "device" {
                if device_criterion_has_adjustment(l) {
                    let anchor = parent_addr
                        .get(&l.campaign)
                        .cloned()
                        .unwrap_or_else(|| format!("campaigns/{}", l.campaign));
                    let detail = match l.bid_modifier {
                        Some(m) => format!("bid_modifier {m}"),
                        None => "negative".to_string(),
                    };
                    warnings.push(format!(
                        "{anchor}: live {descriptor} ({detail}) is not declared, and Google Ads \
                         forbids removing device criteria — leaving it untouched. Declare a \
                         matching device block to manage it (omit bid_modifier to reset the \
                         adjustment)."
                    ));
                }
                continue;
            }
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
        // Still gated on ≥1 declared member: sets match by bare name, so an
        // empty declared set adopting a UI-curated one must not empty it.
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

    (out, warnings)
}

/// True when a live device criterion deviates from its default state — the
/// state Google's auto-materialized criteria are born in (no modifier, not
/// negative). Default-state criteria are implicitly desired, not drift.
fn device_criterion_has_adjustment(cr: &JsonCampaignCriterion) -> bool {
    cr.negative.unwrap_or(false) || cr.bid_modifier.is_some_and(|m| (m - 1.0).abs() > 1e-6)
}

/// A `REMOVED` live resource can no longer be mutated: the Google Ads API
/// rejects every op against it, and one such op sinks the whole atomic batch.
/// Removed resources keep their `bidsmith:address` label, so without this guard
/// a removal that succeeded would re-plan as a doomed destroy on every
/// subsequent plan.
fn is_removed(status: Option<&str>) -> bool {
    status == Some("REMOVED")
}

/// A whole labeled resource that is no longer declared — destroyed because its
/// `bidsmith:address` label proves bidsmith owns it. Unlabeled live resources
/// (UI-created, unmanaged) are never produced here.
fn removal_diff(kind: &'static str, address: &str, live_id: &str) -> ResourceDiff {
    ResourceDiff {
        address: format!("{address} (managed, no longer declared)"),
        kind,
        action: Action::Delete {
            live_id: live_id.to_string(),
        },
    }
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
    } else if cr.device.is_some() {
        "device"
    } else if cr.youtube_channel.is_some() {
        "youtube_channel"
    } else if cr.youtube_video.is_some() {
        "youtube_video"
    } else if cr.topic.is_some() {
        "topic"
    } else if cr.user_interest.is_some() {
        "user_interest"
    } else if cr.age_range.is_some() {
        "age_range"
    } else if cr.gender.is_some() {
        "gender"
    } else if cr.audience.is_some() {
        "audience"
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
    } else if let Some(dev) = &cr.device {
        Some(format!("device {}", dev.ty))
    } else if let Some(c) = &cr.youtube_channel {
        Some(format!("youtube_channel {}", c.channel_id))
    } else if let Some(v) = &cr.youtube_video {
        Some(format!("youtube_video {}", v.video_id))
    } else if let Some(t) = &cr.topic {
        Some(format!("topic {}", t.topic_constant))
    } else if let Some(u) = &cr.user_interest {
        Some(format!("user_interest {}", u.user_interest_category))
    } else if let Some(a) = &cr.age_range {
        Some(format!("age_range {}", a.ty))
    } else if let Some(g) = &cr.gender {
        Some(format!("gender {}", g.ty))
    } else if let Some((field, value)) = cr.audience.as_ref().and_then(JsonAudience::source) {
        Some(format!("{field} {value}"))
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
    if sorted_frequency_caps(d) != sorted_frequency_caps(l) {
        c.push("frequency_caps".into());
    }
    c
}

/// The whole cap list is one API field, so it diffs as a set — reordering the
/// blocks in a `.bid` is not a change.
fn sorted_frequency_caps(c: &JsonCampaign) -> Vec<(&str, &str, &str, i64, i64)> {
    let mut caps: Vec<_> = c.frequency_caps.iter().map(|f| f.sort_key()).collect();
    caps.sort_unstable();
    caps
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

/// Match declared ads to live ads 1:1 within each ad group, keyed on the ad
/// *body* (final URLs + RSA content), not on the ad group alone. Accounts often
/// hold several ads with byte-identical bodies that differ only by status
/// (`adblock_rsa_7/8/9`); keying on the ad group and taking the first live ad
/// collapsed them all onto one live id, which surfaced as spurious status
/// updates, "Cannot mutate the same resource twice" rejections, and bogus
/// creates for the unmatched ads. Each live ad is consumed at most once: within
/// a body bucket we first claim the live ads that already match (no diff), then
/// assign the rest as status updates; a declared ad with no live body left is a
/// create.
fn match_ad_group_ads(
    declared: &[JsonAdGroupAd],
    live: &[JsonAdGroupAd],
    ad_group_match: &HashMap<String, String>,
    asset_match: &HashMap<String, String>,
) -> Vec<(Action, Option<usize>)> {
    let mut consumed = vec![false; live.len()];
    let mut out: Vec<(Action, Option<usize>)> = vec![(Action::Create, None); declared.len()];

    // Pass 0: label hits — a declared ad claims the live ad carrying its
    // address, regardless of body. Body is creation-only, so a body edit on a
    // labeled ad falls through to a create + a destroy of the old ad below.
    let mut live_by_addr: HashMap<&str, usize> = HashMap::new();
    for (i, l) in live.iter().enumerate() {
        if let Some(a) = l.managed_address.as_deref() {
            live_by_addr.entry(a).or_insert(i);
        }
    }
    for (di, d) in declared.iter().enumerate() {
        if let Some(&li) = live_by_addr.get(address_label_payload(&d.id).as_str()) {
            if !consumed[li] && ad_bodies_match(d, &live[li], asset_match) {
                consumed[li] = true;
                out[di] = (
                    action_for_match(live[li].id.clone(), diff_ad_group_ad(d, &live[li])),
                    Some(li),
                );
            }
        }
    }

    // Body 1:1 buckets for the rest, keyed on (live ad_group, body). Buckets are
    // disjoint in both declared and live indices (a distinct body key never
    // shares live ads), so the result is independent of bucket order.
    let mut live_buckets: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (i, l) in live.iter().enumerate() {
        if consumed[i] {
            continue;
        }
        live_buckets
            .entry((l.ad_group.clone(), ad_body_key(l, asset_match)))
            .or_default()
            .push(i);
    }
    let mut declared_buckets: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (i, d) in declared.iter().enumerate() {
        if out[i].1.is_some() {
            continue;
        }
        if let Some(parent_id) = ad_group_match.get(&d.ad_group) {
            declared_buckets
                .entry((parent_id.clone(), ad_body_key(d, asset_match)))
                .or_default()
                .push(i);
        }
    }

    for (key, decl_indices) in &declared_buckets {
        let live_indices = live_buckets.get(key);
        let mut pending: Vec<usize> = Vec::new();
        for &di in decl_indices {
            let claimed = live_indices.and_then(|lis| {
                lis.iter()
                    .copied()
                    .find(|&li| !consumed[li] && diff_ad_group_ad(&declared[di], &live[li]).is_empty())
            });
            match claimed {
                Some(li) => {
                    consumed[li] = true;
                    out[di] = (
                        Action::NoOp {
                            live_id: live[li].id.clone(),
                        },
                        Some(li),
                    );
                }
                None => pending.push(di),
            }
        }
        for di in pending {
            let claimed = live_indices.and_then(|lis| lis.iter().copied().find(|&li| !consumed[li]));
            if let Some(li) = claimed {
                consumed[li] = true;
                out[di] = (
                    action_for_match(live[li].id.clone(), diff_ad_group_ad(&declared[di], &live[li])),
                    Some(li),
                );
            }
        }
    }

    // Last pass: an `ad {}` with no creative block declares the URLs and leaves
    // the creative unmanaged — what a refresh of a UI-built ad renders. It
    // matches any live ad in the group with the same URLs, creative and all,
    // so adopting one never plans as a destroy plus an ad-less create.
    for (di, d) in declared.iter().enumerate() {
        if out[di].1.is_some() || declares_creative(d) {
            continue;
        }
        let Some(parent_id) = ad_group_match.get(&d.ad_group) else {
            continue;
        };
        let claimed = live
            .iter()
            .enumerate()
            .filter(|(li, l)| {
                !consumed[*li] && &l.ad_group == parent_id && ad_urls_key(l) == ad_urls_key(d)
            })
            .map(|(li, _)| li)
            .min_by_key(|&li| usize::from(!diff_ad_group_ad(d, &live[li]).is_empty()));
        if let Some(li) = claimed {
            consumed[li] = true;
            out[di] = (
                action_for_match(live[li].id.clone(), diff_ad_group_ad(d, &live[li])),
                Some(li),
            );
        }
    }

    out
}

fn declares_creative(a: &JsonAdGroupAd) -> bool {
    a.ad.responsive_search_ad.is_some()
        || a.ad.video_responsive_ad.is_some()
        || a.ad.demand_gen_video_responsive_ad.is_some()
}

/// Body match for the label pass: a declared ad that leaves the creative
/// unmanaged compares on URLs alone, everything else on the full body.
fn ad_bodies_match(
    d: &JsonAdGroupAd,
    l: &JsonAdGroupAd,
    asset_match: &HashMap<String, String>,
) -> bool {
    if declares_creative(d) {
        ad_body_key(d, asset_match) == ad_body_key(l, asset_match)
    } else {
        ad_urls_key(d) == ad_urls_key(l)
    }
}

fn ad_urls_key(a: &JsonAdGroupAd) -> String {
    a.ad.final_urls.join("\u{1f}")
}

/// A stable key for an ad's content (everything `diff_ad_group_ad` treats as
/// creation-only). Status is deliberately excluded so identical-bodied ads in
/// different states share a bucket and get assigned 1:1. Video refs run through
/// `asset_match` so a declared asset address and the live asset id it matched
/// produce the same key; a live id is its own key, so one function serves both
/// sides.
fn ad_body_key(a: &JsonAdGroupAd, asset_match: &HashMap<String, String>) -> String {
    use std::fmt::Write;
    let video_id = |r: &String| asset_match.get(r).unwrap_or(r).clone();
    let mut k = String::new();
    let _ = write!(k, "urls:{}", ad_urls_key(a));
    if let Some(rsa) = &a.ad.responsive_search_ad {
        k.push_str("\u{1e}h:");
        for h in &rsa.headlines {
            let _ = write!(k, "{}\u{1f}{}\u{1d}", h.text, h.pin.as_deref().unwrap_or(""));
        }
        k.push_str("\u{1e}d:");
        for d in &rsa.descriptions {
            let _ = write!(k, "{}\u{1f}{}\u{1d}", d.text, d.pin.as_deref().unwrap_or(""));
        }
        let _ = write!(
            k,
            "\u{1e}p1:{}\u{1e}p2:{}",
            rsa.path1.as_deref().unwrap_or(""),
            rsa.path2.as_deref().unwrap_or(""),
        );
    }
    if let Some(v) = &a.ad.video_responsive_ad {
        let _ = write!(
            k,
            "\u{1e}video:{}\u{1e}vh:{}\u{1e}vlh:{}\u{1e}vd:{}\u{1e}vcta:{}",
            video_id(&v.video),
            v.headlines.join("\u{1f}"),
            v.long_headlines.join("\u{1f}"),
            v.descriptions.join("\u{1f}"),
            v.call_to_actions.join("\u{1f}"),
        );
    }
    if let Some(dg) = &a.ad.demand_gen_video_responsive_ad {
        // call_to_actions are left out: live Demand Gen CTAs are asset refs, so
        // reading them back never reproduces the declared text and every ad
        // would look like a body change.
        let videos: Vec<String> = dg.videos.iter().map(video_id).collect();
        let _ = write!(
            k,
            "\u{1e}dgvideos:{}\u{1e}dgh:{}\u{1e}dglh:{}\u{1e}dgd:{}\u{1e}dgb1:{}\u{1e}dgb2:{}\u{1e}dgbn:{}",
            videos.join("\u{1f}"),
            dg.headlines.join("\u{1f}"),
            dg.long_headlines.join("\u{1f}"),
            dg.descriptions.join("\u{1f}"),
            dg.breadcrumb1.as_deref().unwrap_or(""),
            dg.breadcrumb2.as_deref().unwrap_or(""),
            dg.business_name.as_deref().unwrap_or(""),
        );
    }
    k
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
    if bid_modifier_changed(d.bid_modifier, l.bid_modifier) {
        c.push("bid_modifier".into());
    }
    c
}

fn bid_modifier_changed(d: Option<f64>, l: Option<f64>) -> bool {
    match (d, l) {
        (Some(a), Some(b)) => (a - b).abs() > 1e-6,
        (None, None) => false,
        _ => true,
    }
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

// Assets are content-immutable in the Google Ads API: editing a sitelink's text
// mints a new asset. So identity is the full content tuple, and there are no
// in-place field updates — a content change plans as create (of a new asset).
fn sitelink_asset_key(a: &JsonSitelinkAsset) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}",
        a.link_text,
        a.description1.as_deref().unwrap_or(""),
        a.description2.as_deref().unwrap_or(""),
        a.final_urls.join(",")
    )
}

fn structured_snippet_asset_key(a: &JsonStructuredSnippetAsset) -> String {
    format!("{}\u{1f}{}", a.header, a.values.join(","))
}

fn resolve_campaign_id(campaign_match: &HashMap<String, String>, campaign: &str) -> Option<String> {
    campaign_match.get(campaign).cloned().or_else(|| {
        campaign
            .starts_with("customers/")
            .then(|| campaign.rsplit('/').next().map(str::to_string))
            .flatten()
    })
}

fn resolve_ad_group_id(ad_group_match: &HashMap<String, String>, ad_group: &str) -> Option<String> {
    ad_group_match.get(ad_group).cloned().or_else(|| {
        ad_group
            .starts_with("customers/")
            .then(|| ad_group.rsplit('/').next().map(str::to_string))
            .flatten()
    })
}

fn diff_customer_asset(d: &JsonCustomerAsset, l: &JsonCustomerAsset) -> Vec<String> {
    let mut c = Vec::new();
    if d.status != l.status {
        c.push("status".into());
    }
    c
}

fn diff_campaign_asset(d: &JsonCampaignAsset, l: &JsonCampaignAsset) -> Vec<String> {
    let mut c = Vec::new();
    if d.status != l.status {
        c.push("status".into());
    }
    c
}

fn diff_ad_group_asset(d: &JsonAdGroupAsset, l: &JsonAdGroupAsset) -> Vec<String> {
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

fn diff_custom_audience(d: &JsonCustomAudience, l: &JsonCustomAudience) -> Vec<String> {
    let mut c = Vec::new();
    if d.description != l.description && d.description.is_some() {
        c.push("description".into());
    }
    if d.status != l.status {
        c.push("status".into());
    }
    // `type` is creation-only: the API rejects changing what a segment is built from.
    if sorted_members(d) != sorted_members(l) {
        c.push("members".into());
    }
    c
}

/// The member list is one API field, so it diffs as a set — reordering the
/// blocks in a `.bid` is not a change.
fn sorted_members(a: &JsonCustomAudience) -> Vec<(&'static str, &str)> {
    let mut members: Vec<_> = a
        .members
        .iter()
        .filter_map(|m| m.payload().map(|(_, ty, v)| (ty, v)))
        .collect();
    members.sort_unstable();
    members
}

/// The live-comparable token for an audience reference. A declared
/// `google_ads_custom_audience.<name>` resolves through the match to the live
/// id; anything else falls back to the resource name's last segment, so a raw
/// `customers/X/customAudiences/999` and the live row agree on `999`.
fn canonical_audience(value: &str, custom_audience_match: &HashMap<String, String>) -> String {
    match custom_audience_match.get(value) {
        Some(live_id) => live_id.clone(),
        None => value.rsplit('/').next().unwrap_or(value).to_string(),
    }
}

fn campaign_criterion_key(
    cr: &JsonCampaignCriterion,
    custom_audience_match: &HashMap<String, String>,
) -> Option<String> {
    if let Some(kw) = &cr.keyword {
        let polarity = if cr.negative.unwrap_or(false) { "neg" } else { "pos" };
        return Some(format!("kw:{polarity}:{}|{}", kw.match_type, kw.text));
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
    if let Some(d) = &cr.device {
        return Some(format!("dev:{}", d.ty));
    }
    if let Some(c) = &cr.youtube_channel {
        return Some(format!("ytchan:{}", c.channel_id));
    }
    if let Some(v) = &cr.youtube_video {
        return Some(format!("ytvid:{}", v.video_id));
    }
    if let Some(t) = &cr.topic {
        return Some(format!("topic:{}", t.topic_constant));
    }
    if let Some(u) = &cr.user_interest {
        return Some(format!("interest:{}", u.user_interest_category));
    }
    if let Some(a) = &cr.age_range {
        return Some(format!("age:{}", a.ty));
    }
    if let Some(g) = &cr.gender {
        return Some(format!("gender:{}", g.ty));
    }
    if let Some((field, value)) = cr.audience.as_ref().and_then(JsonAudience::source) {
        return Some(format!(
            "{field}:{}",
            canonical_audience(value, custom_audience_match)
        ));
    }
    None
}

#[cfg(test)]
mod ad_match_tests {
    use super::*;
    use crate::commands::export::{JsonAd, JsonResponsiveSearchAd, JsonRsaAsset};

    fn ad(id: &str, ad_group: &str, status: &str, headline: &str) -> JsonAdGroupAd {
        JsonAdGroupAd {
            id: id.to_string(),
            ad_group: ad_group.to_string(),
            status: Some(status.to_string()),
            ad: JsonAd {
                name: None,
                final_urls: vec!["https://example.com".to_string()],
                responsive_search_ad: Some(JsonResponsiveSearchAd {
                    headlines: vec![JsonRsaAsset {
                        text: headline.to_string(),
                        pin: None,
                    }],
                    descriptions: vec![JsonRsaAsset {
                        text: "Block ads everywhere.".to_string(),
                        pin: None,
                    }],
                    path1: None,
                    path2: None,
                }),
                video_responsive_ad: None,
                demand_gen_video_responsive_ad: None,
            },
            managed_address: None,
        }
    }

    fn identity_match() -> HashMap<String, String> {
        // declared resources reference the same live ad-group id directly.
        let mut m = HashMap::new();
        m.insert("ag".to_string(), "ag".to_string());
        m
    }

    #[test]
    fn identical_bodies_match_one_to_one_after_refresh() {
        // Three same-bodied ads differing only by status — the refresh scenario.
        // Each declared ad must claim a distinct live id with no diff.
        let declared = vec![
            ad("rsa_7", "ag", "ENABLED", "Block ads now"),
            ad("rsa_8", "ag", "PAUSED", "Block ads now"),
            ad("rsa_9", "ag", "ENABLED", "Block ads now"),
        ];
        let live = vec![
            ad("100", "ag", "ENABLED", "Block ads now"),
            ad("101", "ag", "PAUSED", "Block ads now"),
            ad("102", "ag", "ENABLED", "Block ads now"),
        ];

        let actions: Vec<Action> =
            match_ad_group_ads(&declared, &live, &identity_match(), &HashMap::new())
                .into_iter()
                .map(|(a, _)| a)
                .collect();

        assert!(
            actions.iter().all(|a| matches!(a, Action::NoOp { .. })),
            "expected all no-ops, got {actions:?}"
        );
        let mut ids: Vec<&str> = actions.iter().filter_map(|a| a.live_id()).collect();
        ids.sort();
        assert_eq!(ids, vec!["100", "101", "102"], "each live ad claimed once");
    }

    #[test]
    fn exact_status_preferred_over_status_update() {
        // declared [ENABLED, PAUSED] vs live [PAUSED, ENABLED]: a naive
        // first-come match would update both; correct is two no-ops.
        let declared = vec![
            ad("a", "ag", "ENABLED", "Same body"),
            ad("b", "ag", "PAUSED", "Same body"),
        ];
        let live = vec![
            ad("200", "ag", "PAUSED", "Same body"),
            ad("201", "ag", "ENABLED", "Same body"),
        ];

        let actions: Vec<Action> =
            match_ad_group_ads(&declared, &live, &identity_match(), &HashMap::new())
                .into_iter()
                .map(|(a, _)| a)
                .collect();

        assert!(matches!(&actions[0], Action::NoOp { live_id } if live_id == "201"));
        assert!(matches!(&actions[1], Action::NoOp { live_id } if live_id == "200"));
    }

    #[test]
    fn leftover_declared_status_becomes_update() {
        // declared [ENABLED, ENABLED] vs live [ENABLED, PAUSED]: one no-op, one
        // genuine status update — never two updates on the same id.
        let declared = vec![
            ad("a", "ag", "ENABLED", "Same body"),
            ad("b", "ag", "ENABLED", "Same body"),
        ];
        let live = vec![
            ad("300", "ag", "ENABLED", "Same body"),
            ad("301", "ag", "PAUSED", "Same body"),
        ];

        let actions: Vec<Action> =
            match_ad_group_ads(&declared, &live, &identity_match(), &HashMap::new())
                .into_iter()
                .map(|(a, _)| a)
                .collect();

        let noops = actions.iter().filter(|a| matches!(a, Action::NoOp { .. })).count();
        let updates = actions
            .iter()
            .filter(|a| matches!(a, Action::Update { .. }))
            .count();
        assert_eq!((noops, updates), (1, 1));
        let mut ids: Vec<&str> = actions.iter().filter_map(|a| a.live_id()).collect();
        ids.sort();
        assert_eq!(ids, vec!["300", "301"], "no live id reused");
    }

    #[test]
    fn distinct_bodies_do_not_cross_match() {
        // A unique-bodied declared ad with no live counterpart is a create, not a
        // false match against an unrelated live ad in the same group.
        let declared = vec![ad("new", "ag", "ENABLED", "Brand new copy")];
        let live = vec![ad("400", "ag", "ENABLED", "Old copy")];

        let actions: Vec<Action> =
            match_ad_group_ads(&declared, &live, &identity_match(), &HashMap::new())
                .into_iter()
                .map(|(a, _)| a)
                .collect();

        assert!(matches!(&actions[0], Action::Create));
    }

    #[test]
    fn unmapped_ad_group_is_create() {
        let declared = vec![ad("x", "missing_ag", "ENABLED", "Copy")];
        let live = vec![ad("500", "ag", "ENABLED", "Copy")];

        let actions: Vec<Action> =
            match_ad_group_ads(&declared, &live, &identity_match(), &HashMap::new())
                .into_iter()
                .map(|(a, _)| a)
                .collect();

        assert!(matches!(&actions[0], Action::Create));
    }
}

#[cfg(test)]
mod criterion_match_tests {
    use super::*;

    fn input(json: &str) -> ExportInput {
        serde_json::from_str(json).expect("valid test input")
    }

    #[test]
    fn positive_and_negative_same_keyword_do_not_collide() {
        // A live ad group can hold the same keyword as both a positive and a
        // negative criterion. Declaring only the positive must match the live
        // positive and leave the negative untouched — never delete the positive.
        let declared = input(
            r#"{
            "customer_id": "1",
            "campaigns": [{"id":"c","name":"C","advertising_channel_type":"SEARCH","campaign_budget":"b"}],
            "ad_groups": [{"id":"g","name":"G","campaign":"c"}],
            "ad_group_criteria": [
                {"id":"k","ad_group":"g","negative":false,"keyword":{"text":"shoes","match_type":"BROAD"}}
            ]
        }"#,
        );
        let live = input(
            r#"{
            "customer_id": "1",
            "campaigns": [{"id":"100","name":"C","advertising_channel_type":"SEARCH","campaign_budget":"200"}],
            "ad_groups": [{"id":"300","name":"G","campaign":"100"}],
            "ad_group_criteria": [
                {"id":"400","ad_group":"300","negative":false,"keyword":{"text":"shoes","match_type":"BROAD"}},
                {"id":"401","ad_group":"300","negative":true,"keyword":{"text":"shoes","match_type":"BROAD"}}
            ]
        }"#,
        );

        let report = diff(&declared, &live);

        assert_eq!(
            report.delete_count, 0,
            "no criterion should be deleted: {:?}",
            report
                .diffs
                .iter()
                .map(|d| (&d.address, &d.action))
                .collect::<Vec<_>>()
        );
        let crit = report
            .diffs
            .iter()
            .find(|d| d.kind == "ad_group_criterion" && d.address == "k")
            .expect("declared criterion present");
        assert!(
            matches!(&crit.action, Action::NoOp { live_id } if live_id == "400"),
            "declared positive keyword should match the live positive one, got {:?}",
            crit.action
        );
    }

    fn device_criteria(declared_extra: &str, live_extra: &str) -> DiffReport {
        let declared = input(&format!(
            r#"{{
            "customer_id": "1",
            "campaigns": [{{"id":"c","name":"C","advertising_channel_type":"SEARCH","campaign_budget":"b"}}],
            "campaign_criteria": [{declared_extra}]
        }}"#,
        ));
        let live = input(&format!(
            r#"{{
            "customer_id": "1",
            "campaigns": [{{"id":"100","name":"C","advertising_channel_type":"SEARCH","campaign_budget":"200"}}],
            "campaign_criteria": [{live_extra}]
        }}"#,
        ));
        diff(&declared, &live)
    }

    fn crit_action(report: &DiffReport, address: &str) -> Action {
        report
            .diffs
            .iter()
            .find(|d| d.kind == "campaign_criterion" && d.address == address)
            .unwrap_or_else(|| panic!("criterion {address} present"))
            .action
            .clone()
    }

    #[test]
    fn device_criterion_matches_by_type_and_diffs_bid_modifier() {
        // Same device type, same modifier → no-op; changed modifier → in-place
        // update (device type is the match key, bid_modifier a scalar field).
        let same = device_criteria(
            r#"{"id":"d","campaign":"c","bid_modifier":0.0,"device":{"type":"MOBILE"}}"#,
            r#"{"id":"500","campaign":"100","bid_modifier":0.0,"device":{"type":"MOBILE"}}"#,
        );
        assert!(
            matches!(crit_action(&same, "d"), Action::NoOp { live_id } if live_id == "500"),
            "identical device modifier should be a no-op, got {:?}",
            crit_action(&same, "d")
        );

        let changed = device_criteria(
            r#"{"id":"d","campaign":"c","bid_modifier":0.7,"device":{"type":"MOBILE"}}"#,
            r#"{"id":"500","campaign":"100","bid_modifier":0.0,"device":{"type":"MOBILE"}}"#,
        );
        assert!(
            matches!(
                crit_action(&changed, "d"),
                Action::Update { ref changed_fields, .. } if changed_fields == &["bid_modifier".to_string()]
            ),
            "changed device modifier should update bid_modifier, got {:?}",
            crit_action(&changed, "d")
        );
    }

    #[test]
    fn device_criterion_of_new_type_is_created_not_matched() {
        // A declared DESKTOP modifier must not match a live MOBILE one.
        let report = device_criteria(
            r#"{"id":"d","campaign":"c","bid_modifier":1.2,"device":{"type":"DESKTOP"}}"#,
            r#"{"id":"500","campaign":"100","bid_modifier":0.0,"device":{"type":"MOBILE"}}"#,
        );
        assert!(
            matches!(crit_action(&report, "d"), Action::Create),
            "a different device type should create, got {:?}",
            crit_action(&report, "d")
        );
    }

    #[test]
    fn auto_materialized_device_criteria_are_not_destroyed() {
        // Issue #82: a desktop-only campaign declares MOBILE/TABLET at 0 and no
        // DESKTOP; Google auto-materializes DESKTOP (id 30000) in default state.
        // It must read as implicitly desired — no doomed destroy, no warning.
        let report = device_criteria(
            r#"{"id":"m","campaign":"c","bid_modifier":0.0,"device":{"type":"MOBILE"}},
               {"id":"t","campaign":"c","bid_modifier":0.0,"device":{"type":"TABLET"}}"#,
            r#"{"id":"30000","campaign":"100","device":{"type":"DESKTOP"}},
               {"id":"30001","campaign":"100","bid_modifier":0.0,"device":{"type":"MOBILE"}},
               {"id":"30002","campaign":"100","bid_modifier":0.0,"device":{"type":"TABLET"}},
               {"id":"30004","campaign":"100","bid_modifier":1.0,"device":{"type":"CONNECTED_TV"}}"#,
        );
        assert_eq!(
            report.delete_count, 0,
            "device criteria must never be destroyed: {:?}",
            report
                .diffs
                .iter()
                .map(|d| (&d.address, &d.action))
                .collect::<Vec<_>>()
        );
        assert!(report.warnings.is_empty(), "default-state device criteria are not drift: {:?}", report.warnings);
        assert!(matches!(crit_action(&report, "m"), Action::NoOp { .. }));
        assert!(matches!(crit_action(&report, "t"), Action::NoOp { .. }));
    }

    #[test]
    fn undeclared_device_adjustment_warns_instead_of_destroying() {
        let report = device_criteria(
            r#"{"id":"m","campaign":"c","bid_modifier":0.0,"device":{"type":"MOBILE"}}"#,
            r#"{"id":"30001","campaign":"100","bid_modifier":0.0,"device":{"type":"MOBILE"}},
               {"id":"30002","campaign":"100","bid_modifier":0.7,"device":{"type":"TABLET"}}"#,
        );
        assert_eq!(report.delete_count, 0, "even a drifted device criterion must not be destroyed");
        assert_eq!(report.warnings.len(), 1, "drift should surface as a warning: {:?}", report.warnings);
        assert!(
            report.warnings[0].contains("device TABLET") && report.warnings[0].contains("0.7"),
            "warning should name the criterion and modifier: {}",
            report.warnings[0]
        );
    }
}

#[cfg(test)]
mod label_match_tests {
    use super::*;

    fn input(json: &str) -> ExportInput {
        serde_json::from_str(json).expect("valid test input")
    }

    const DECLARED_SUMMER: &str = r#"{
        "customer_id": "100",
        "campaign_budgets": [{"id":"m.b","name":"B","amount_micros":1000}],
        "campaigns": [{"id":"m.google_ads_campaign.summer","name":"Summer","advertising_channel_type":"SEARCH","campaign_budget":"m.b"}]
    }"#;

    fn campaign_diff(report: &DiffReport) -> &ResourceDiff {
        report
            .diffs
            .iter()
            .find(|d| d.kind == "campaign")
            .expect("a campaign diff")
    }

    #[test]
    fn adoption_labels_an_unlabeled_match() {
        // Live campaign matches by name but carries no bidsmith label: adopt it
        // (no field change) and schedule a fresh label write.
        let live = input(
            r#"{
            "customer_id": "100",
            "campaign_budgets": [{"id":"999","name":"B","amount_micros":1000}],
            "campaigns": [{"id":"555","name":"Summer","advertising_channel_type":"SEARCH","campaign_budget":"999"}]
        }"#,
        );
        let report = diff(&input(DECLARED_SUMMER), &live);

        assert!(matches!(&campaign_diff(&report).action, Action::NoOp { live_id } if live_id == "555"));
        assert_eq!(report.adopt_count, 1);
        let plan = report
            .label_plans
            .iter()
            .find(|p| p.kind == "campaign")
            .expect("a label plan");
        assert_eq!(plan.label_address, "m.google_ads_campaign.summer");
        assert!(plan.existing_label_rn.is_none(), "fresh label is created");
        assert!(plan.stale_assoc_rn.is_none());
    }

    #[test]
    fn correct_label_is_a_clean_noop() {
        let live = input(
            r#"{
            "customer_id": "100",
            "campaign_budgets": [{"id":"999","name":"B","amount_micros":1000}],
            "campaigns": [{"id":"555","name":"Summer","advertising_channel_type":"SEARCH","campaign_budget":"999","managed_address":"m.google_ads_campaign.summer"}],
            "labels": {"m.google_ads_campaign.summer":"customers/100/labels/777"}
        }"#,
        );
        let report = diff(&input(DECLARED_SUMMER), &live);

        assert!(matches!(&campaign_diff(&report).action, Action::NoOp { .. }));
        assert_eq!(report.adopt_count, 0);
        assert!(report.label_plans.is_empty(), "label already correct");
    }

    #[test]
    fn renaming_the_name_on_a_labeled_campaign_is_an_update() {
        // The live campaign is identified by its label, so changing its display
        // name is an in-place update — not a create + orphan.
        let live = input(
            r#"{
            "customer_id": "100",
            "campaign_budgets": [{"id":"999","name":"B","amount_micros":1000}],
            "campaigns": [{"id":"555","name":"Old Name","advertising_channel_type":"SEARCH","campaign_budget":"999","managed_address":"m.google_ads_campaign.summer"}],
            "labels": {"m.google_ads_campaign.summer":"customers/100/labels/777"}
        }"#,
        );
        let report = diff(&input(DECLARED_SUMMER), &live);

        assert!(
            matches!(&campaign_diff(&report).action, Action::Update { changed_fields, .. } if changed_fields.iter().any(|f| f == "name")),
            "expected a name update, got {:?}",
            campaign_diff(&report).action
        );
        assert_eq!(report.create_count, 0);
        assert!(report.label_plans.is_empty(), "label already correct");
    }

    #[test]
    fn labeled_orphan_is_destroyed_unlabeled_is_left_alone() {
        // Two undeclared live campaigns: the bidsmith-labeled one is destroyed,
        // the UI-created (unlabeled) one is untouched.
        let declared = input(r#"{"customer_id":"100","campaign_budgets":[{"id":"m.b","name":"B","amount_micros":1000}]}"#);
        let live = input(
            r#"{
            "customer_id": "100",
            "campaigns": [
                {"id":"555","name":"Managed","advertising_channel_type":"SEARCH","campaign_budget":"999","managed_address":"m.google_ads_campaign.gone"},
                {"id":"556","name":"Hand Made","advertising_channel_type":"SEARCH","campaign_budget":"999"}
            ],
            "labels": {"m.google_ads_campaign.gone":"customers/100/labels/777"}
        }"#,
        );
        let report = diff(&declared, &live);

        let destroys: Vec<&ResourceDiff> = report
            .diffs
            .iter()
            .filter(|d| d.kind == "campaign" && matches!(d.action, Action::Delete { .. }))
            .collect();
        assert_eq!(report.delete_count, 1);
        assert_eq!(destroys.len(), 1);
        assert!(matches!(&destroys[0].action, Action::Delete { live_id } if live_id == "555"));
    }

    #[test]
    fn address_rename_relabels_via_content_fallback() {
        // `bidsmith mv` changed the address in source (summer -> autumn) but the
        // live campaign still carries the old label and the same name. Content
        // fallback re-adopts it under the new address and reconciles the label —
        // no destroy, no recreate.
        let declared = input(
            r#"{
            "customer_id": "100",
            "campaign_budgets": [{"id":"m.b","name":"B","amount_micros":1000}],
            "campaigns": [{"id":"m.google_ads_campaign.autumn","name":"Summer","advertising_channel_type":"SEARCH","campaign_budget":"m.b"}]
        }"#,
        );
        let live = input(
            r#"{
            "customer_id": "100",
            "campaign_budgets": [{"id":"999","name":"B","amount_micros":1000}],
            "campaigns": [{"id":"555","name":"Summer","advertising_channel_type":"SEARCH","campaign_budget":"999","managed_address":"m.google_ads_campaign.summer"}],
            "labels": {"m.google_ads_campaign.summer":"customers/100/labels/777"}
        }"#,
        );
        let report = diff(&declared, &live);

        assert!(matches!(&campaign_diff(&report).action, Action::NoOp { live_id } if live_id == "555"));
        assert_eq!(report.delete_count, 0, "mv must never destroy");
        let plan = report
            .label_plans
            .iter()
            .find(|p| p.kind == "campaign")
            .expect("a relabel plan");
        assert_eq!(plan.label_address, "m.google_ads_campaign.autumn");
        assert_eq!(
            plan.stale_assoc_rn.as_deref(),
            Some("customers/100/campaignLabels/555~777"),
            "the old label association is removed"
        );
    }

    const LONG_ADDR: &str =
        "instream.google_ads_campaign.instream_preroll_a_rather_long_campaign_family_identifier_2026";

    fn long_declared() -> String {
        format!(
            r#"{{
            "customer_id": "100",
            "campaign_budgets": [{{"id":"m.b","name":"B","amount_micros":1000}}],
            "campaigns": [{{"id":"{LONG_ADDR}","name":"Preroll","advertising_channel_type":"SEARCH","campaign_budget":"m.b"}}]
        }}"#
        )
    }

    #[test]
    fn adopting_a_long_address_writes_a_label_that_fits_eighty_chars() {
        // The reported bug: an unlabeled live campaign whose address overflows the
        // 80-char label cap. Adoption must encode the label so the API accepts it.
        let live = input(
            r#"{
            "customer_id": "100",
            "campaign_budgets": [{"id":"999","name":"B","amount_micros":1000}],
            "campaigns": [{"id":"555","name":"Preroll","advertising_channel_type":"SEARCH","campaign_budget":"999"}]
        }"#,
        );
        let report = diff(&input(&long_declared()), &live);

        assert!(matches!(&campaign_diff(&report).action, Action::NoOp { live_id } if live_id == "555"));
        assert_eq!(report.adopt_count, 1);
        let plan = report
            .label_plans
            .iter()
            .find(|p| p.kind == "campaign")
            .expect("a label plan");
        let label_name = format!(
            "{}{}",
            crate::commands::export::ADDRESS_LABEL_PREFIX,
            plan.label_address
        );
        assert!(
            label_name.len() <= crate::commands::export::MAX_LABEL_NAME_LEN,
            "label {label_name:?} is {} chars",
            label_name.len()
        );
        assert_ne!(plan.label_address, LONG_ADDR, "the long address is encoded");
    }

    #[test]
    fn a_long_address_already_labeled_is_a_clean_noop() {
        // Second run after adoption: the live campaign carries the encoded payload.
        // It must read as already-labeled, not re-adopt forever.
        let payload = address_label_payload(LONG_ADDR);
        let live = input(&format!(
            r#"{{
            "customer_id": "100",
            "campaign_budgets": [{{"id":"999","name":"B","amount_micros":1000}}],
            "campaigns": [{{"id":"555","name":"Preroll","advertising_channel_type":"SEARCH","campaign_budget":"999","managed_address":"{payload}"}}],
            "labels": {{"{payload}":"customers/100/labels/777"}}
        }}"#
        ));
        let report = diff(&input(&long_declared()), &live);

        assert!(matches!(&campaign_diff(&report).action, Action::NoOp { .. }));
        assert_eq!(report.adopt_count, 0, "must not re-adopt an already-labeled resource");
        assert!(report.label_plans.is_empty(), "label already correct");
    }
}

#[cfg(test)]
mod removed_resource_tests {
    use super::*;

    fn input(json: &str) -> ExportInput {
        serde_json::from_str(json).expect("valid test input")
    }

    fn ad_destroys(report: &DiffReport) -> Vec<&str> {
        report
            .diffs
            .iter()
            .filter(|d| d.kind == "ad_group_ad" && matches!(d.action, Action::Delete { .. }))
            .filter_map(|d| d.action.live_id())
            .collect()
    }

    #[test]
    fn a_video_ad_matching_live_plans_as_a_no_op() {
        // Second plan after `apply` created the asset and the creative: the
        // declared video ref is an address, live reports an asset id, and the
        // two have to reconcile or every plan would re-create the ad.
        let declared = input(
            r#"{
            "customer_id": "100",
            "ad_groups": [{"id":"m.ag","name":"In-stream","campaign":"m.c"}],
            "youtube_video_assets": [{"id":"m.brand","youtube_video_id":"dQw4w9WgXcQ"}],
            "ad_group_ads": [{
                "id":"m.preroll","ad_group":"m.ag","status":"PAUSED",
                "ad":{"final_urls":["https://e.com"],"video_responsive_ad":{
                    "video":"m.brand","headlines":["Block ads"],"call_to_actions":["Install"]
                }}
            }]
        }"#,
        );
        let live = input(
            r#"{
            "customer_id": "100",
            "ad_groups": [{"id":"55","name":"In-stream","campaign":"c1","managed_address":"m.ag"}],
            "youtube_video_assets": [{"id":"42","youtube_video_id":"dQw4w9WgXcQ"}],
            "ad_group_ads": [{
                "id":"55~9","ad_group":"55","status":"PAUSED",
                "ad":{"final_urls":["https://e.com"],"video_responsive_ad":{
                    "video":"42","headlines":["Block ads"],"call_to_actions":["Install"]
                }},
                "managed_address":"m.preroll"
            }],
            "labels": {"m.ag":"customers/100/labels/1","m.preroll":"customers/100/labels/2"}
        }"#,
        );
        let report = diff(&declared, &live);

        let ad = report
            .diffs
            .iter()
            .find(|d| d.address == "m.preroll")
            .expect("the video ad is diffed");
        assert!(
            matches!(ad.action, Action::NoOp { .. }),
            "an unchanged video ad must not re-create: {:?}",
            ad.action
        );
        let asset = report
            .diffs
            .iter()
            .find(|d| d.kind == "youtube_video_asset")
            .expect("the video asset is diffed");
        assert_eq!(asset.action.live_id(), Some("42"));
        assert!(ad_destroys(&report).is_empty());
    }

    #[test]
    fn a_scaffold_only_ad_still_adopts_a_ui_built_creative() {
        // The shape `refresh` produced for a UI-built video ad before the
        // creative was readable: URLs, no creative block. It has to keep
        // adopting the live ad rather than destroying it and creating an
        // ad with no creative at all.
        let declared = input(
            r#"{
            "customer_id": "100",
            "ad_groups": [{"id":"m.ag","name":"In-stream","campaign":"m.c"}],
            "ad_group_ads": [{
                "id":"m.preroll","ad_group":"m.ag","status":"PAUSED",
                "ad":{"final_urls":["https://e.com"]}
            }]
        }"#,
        );
        let live = input(
            r#"{
            "customer_id": "100",
            "ad_groups": [{"id":"55","name":"In-stream","campaign":"c1","managed_address":"m.ag"}],
            "ad_group_ads": [{
                "id":"55~9","ad_group":"55","status":"PAUSED",
                "ad":{"final_urls":["https://e.com"],"video_responsive_ad":{
                    "video":"42","headlines":["Built in the UI"]
                }},
                "managed_address":"m.preroll"
            }],
            "labels": {"m.ag":"customers/100/labels/1","m.preroll":"customers/100/labels/2"}
        }"#,
        );
        let report = diff(&declared, &live);

        let ad = report
            .diffs
            .iter()
            .find(|d| d.address == "m.preroll")
            .expect("the ad is diffed");
        assert_eq!(ad.action.live_id(), Some("55~9"), "{:?}", ad.action);
        assert!(ad_destroys(&report).is_empty());
    }

    #[test]
    fn an_unlabeled_ui_built_creative_adopts_onto_a_scaffold_only_ad() {
        let declared = input(
            r#"{
            "customer_id": "100",
            "ad_groups": [{"id":"m.ag","name":"In-stream","campaign":"m.c"}],
            "ad_group_ads": [{
                "id":"m.preroll","ad_group":"m.ag","status":"PAUSED",
                "ad":{"final_urls":["https://e.com"]}
            }]
        }"#,
        );
        let live = input(
            r#"{
            "customer_id": "100",
            "ad_groups": [{"id":"55","name":"In-stream","campaign":"c1","managed_address":"m.ag"}],
            "ad_group_ads": [{
                "id":"55~9","ad_group":"55","status":"PAUSED",
                "ad":{"final_urls":["https://e.com"],"video_responsive_ad":{
                    "video":"42","headlines":["Built in the UI"]
                }}
            }],
            "labels": {"m.ag":"customers/100/labels/1"}
        }"#,
        );
        let report = diff(&declared, &live);

        let ad = report
            .diffs
            .iter()
            .find(|d| d.address == "m.preroll")
            .expect("the ad is diffed");
        assert_eq!(ad.action.live_id(), Some("55~9"), "{:?}", ad.action);
    }

    #[test]
    fn a_changed_video_creative_replaces_the_ad() {
        let declared = input(
            r#"{
            "customer_id": "100",
            "ad_groups": [{"id":"m.ag","name":"In-stream","campaign":"m.c"}],
            "youtube_video_assets": [{"id":"m.brand","youtube_video_id":"dQw4w9WgXcQ"}],
            "ad_group_ads": [{
                "id":"m.preroll","ad_group":"m.ag","status":"PAUSED",
                "ad":{"final_urls":["https://e.com"],"video_responsive_ad":{
                    "video":"m.brand","headlines":["Block trackers too"]
                }}
            }]
        }"#,
        );
        let live = input(
            r#"{
            "customer_id": "100",
            "ad_groups": [{"id":"55","name":"In-stream","campaign":"c1","managed_address":"m.ag"}],
            "youtube_video_assets": [{"id":"42","youtube_video_id":"dQw4w9WgXcQ"}],
            "ad_group_ads": [{
                "id":"55~9","ad_group":"55","status":"PAUSED",
                "ad":{"final_urls":["https://e.com"],"video_responsive_ad":{
                    "video":"42","headlines":["Block ads"]
                }},
                "managed_address":"m.preroll"
            }],
            "labels": {"m.ag":"customers/100/labels/1","m.preroll":"customers/100/labels/2"}
        }"#,
        );
        let report = diff(&declared, &live);

        let ad = report
            .diffs
            .iter()
            .find(|d| d.address == "m.preroll")
            .expect("the video ad is diffed");
        assert!(
            matches!(ad.action, Action::Create),
            "an edited creative is a new ad: {:?}",
            ad.action
        );
        assert_eq!(ad_destroys(&report), vec!["55~9"]);
    }

    #[test]
    fn ads_orphaned_under_a_removed_ad_group_are_not_re_destroyed() {
        // Second plan after a whole ad group was removed: the group is gone from
        // live state but its ads survive ENABLED, still carrying their labels.
        // The orphan must be skipped (its destroy would reject with "Removed ads
        // may not be modified" and sink the batch); a normal undeclared ad under
        // a still-live group is still destroyed.
        let declared = input(r#"{"customer_id":"100"}"#);
        let live = input(
            r#"{
            "customer_id": "100",
            "ad_groups": [
                {"id":"live_ag","name":"Live","campaign":"c1"}
            ],
            "ad_group_ads": [
                {"id":"live_ag~ad1","ad_group":"live_ag","status":"ENABLED","ad":{"final_urls":["https://e.com"]},"managed_address":"m.google_ads_ad_group_ad.under_live"},
                {"id":"gone_ag~ad2","ad_group":"gone_ag","status":"ENABLED","ad":{"final_urls":["https://e.com"]},"managed_address":"m.google_ads_ad_group_ad.orphan"}
            ],
            "labels": {
                "m.google_ads_ad_group_ad.under_live":"customers/100/labels/2",
                "m.google_ads_ad_group_ad.orphan":"customers/100/labels/3"
            }
        }"#,
        );
        let report = diff(&declared, &live);

        assert_eq!(
            ad_destroys(&report),
            vec!["live_ag~ad1"],
            "orphan under a removed ad group must be skipped; ad under a live group still destroyed"
        );
    }

    #[test]
    fn a_removed_status_ad_is_not_re_destroyed() {
        // Defense in depth: a REMOVED ad leaking into live state (its label
        // survives removal) must not re-plan as a destroy, even under a live group.
        let declared = input(r#"{"customer_id":"100"}"#);
        let live = input(
            r#"{
            "customer_id": "100",
            "ad_groups": [
                {"id":"live_ag","name":"Live","campaign":"c1"}
            ],
            "ad_group_ads": [
                {"id":"live_ag~ad1","ad_group":"live_ag","status":"REMOVED","ad":{"final_urls":["https://e.com"]},"managed_address":"m.google_ads_ad_group_ad.dead"}
            ],
            "labels": {"m.google_ads_ad_group_ad.dead":"customers/100/labels/2"}
        }"#,
        );
        let report = diff(&declared, &live);

        assert!(
            ad_destroys(&report).is_empty(),
            "a REMOVED ad must not be re-flagged for destroy"
        );
    }
}

#[cfg(test)]
mod claim_tests {
    use super::*;

    fn input(json: &str) -> ExportInput {
        serde_json::from_str(json).expect("valid test input")
    }

    // A campaign + ad group declaring only positive keywords — the state after
    // removing the last negative-keyword resource (issue #88).
    const DECLARED_POSITIVES_ONLY: &str = r#"{
        "customer_id": "1",
        "campaigns": [{"id":"m.c","name":"C","advertising_channel_type":"SEARCH","campaign_budget":"m.b"}],
        "ad_groups": [{"id":"m.g","name":"G","campaign":"m.c"}],
        "ad_group_criteria": [
            {"id":"m.k","ad_group":"m.g","negative":false,"keyword":{"text":"shoes","match_type":"EXACT"}}
        ]
    }"#;

    fn live_with_orphaned_negatives(claims_and_labels: &str) -> ExportInput {
        input(&format!(
            r#"{{
            "customer_id": "1",
            "campaigns": [{{"id":"100","name":"C","advertising_channel_type":"SEARCH","campaign_budget":"200"}}],
            "ad_groups": [{{"id":"300","name":"G","campaign":"100"}}],
            "ad_group_criteria": [
                {{"id":"400","ad_group":"300","negative":false,"keyword":{{"text":"shoes","match_type":"EXACT"}}}},
                {{"id":"401","ad_group":"300","negative":true,"keyword":{{"text":"vpn","match_type":"EXACT"}}}},
                {{"id":"402","ad_group":"300","negative":true,"keyword":{{"text":"proxy","match_type":"PHRASE"}}}}
            ]{claims_and_labels}
        }}"#
        ))
    }

    #[test]
    fn live_claim_keeps_destroying_after_last_declared_member_is_removed() {
        // Issue #88: the negatives resource is gone from desired state, but the
        // ad group's live `bidsmith:owns=keyword_negative` claim proves bidsmith
        // managed that category — the orphans must plan as destroys and the
        // claim must be released in the same batch.
        let live = live_with_orphaned_negatives(
            r#",
            "ad_group_claims": {"300": ["keyword_negative", "keyword_positive"]},
            "claim_labels": {
                "keyword_negative": "customers/1/labels/777",
                "keyword_positive": "customers/1/labels/778"
            }"#,
        );
        let report = diff(&input(DECLARED_POSITIVES_ONLY), &live);

        let destroyed: Vec<&ResourceDiff> = report
            .diffs
            .iter()
            .filter(|d| matches!(d.action, Action::Delete { .. }))
            .collect();
        assert_eq!(
            destroyed.len(),
            2,
            "both orphaned negatives should be destroyed: {:?}",
            report.diffs.iter().map(|d| (&d.address, &d.action)).collect::<Vec<_>>()
        );
        assert!(destroyed
            .iter()
            .all(|d| d.kind == "ad_group_criterion" && d.address.contains("m.g (removed negative_keyword")));

        let releases: Vec<&ClaimPlanEntry> = report
            .claim_plans
            .iter()
            .filter(|p| p.stale_assoc_rn.is_some())
            .collect();
        assert_eq!(releases.len(), 1, "claims: {:?}", report.claim_plans);
        assert_eq!(releases[0].category, "keyword_negative");
        assert_eq!(
            releases[0].stale_assoc_rn.as_deref(),
            Some("customers/1/adGroupLabels/300~777")
        );
        assert!(
            !report
                .claim_plans
                .iter()
                .any(|p| p.category == "keyword_positive"),
            "the still-declared positive claim already exists live: {:?}",
            report.claim_plans
        );
    }

    #[test]
    fn unclaimed_live_negatives_are_never_destroyed() {
        // Adoption safety: no declared negatives, no live claim — the negatives
        // could be UI-managed, so they stay. The declared positive category
        // plans a fresh claim instead.
        let live = live_with_orphaned_negatives("");
        let report = diff(&input(DECLARED_POSITIVES_ONLY), &live);

        assert_eq!(
            report.delete_count, 0,
            "unclaimed live negatives must be left alone: {:?}",
            report.diffs.iter().map(|d| (&d.address, &d.action)).collect::<Vec<_>>()
        );
        let adds: Vec<&ClaimPlanEntry> = report
            .claim_plans
            .iter()
            .filter(|p| p.stale_assoc_rn.is_none())
            .collect();
        assert_eq!(adds.len(), 1, "claims: {:?}", report.claim_plans);
        assert_eq!(adds[0].kind, "ad_group");
        assert_eq!(adds[0].address, "m.g");
        assert_eq!(adds[0].category, "keyword_positive");
        assert!(adds[0].existing_label_rn.is_none(), "no live owns label yet");
    }

    #[test]
    fn claim_add_reuses_an_existing_owns_label() {
        let live = live_with_orphaned_negatives(
            r#",
            "claim_labels": {"keyword_positive": "customers/1/labels/778"}"#,
        );
        let report = diff(&input(DECLARED_POSITIVES_ONLY), &live);

        let add = report
            .claim_plans
            .iter()
            .find(|p| p.stale_assoc_rn.is_none())
            .expect("a claim add");
        assert_eq!(
            add.existing_label_rn.as_deref(),
            Some("customers/1/labels/778")
        );
    }

    #[test]
    fn stale_claim_without_live_members_plans_only_a_release() {
        let declared = input(
            r#"{
            "customer_id": "1",
            "campaigns": [{"id":"m.c","name":"C","advertising_channel_type":"SEARCH","campaign_budget":"m.b"}],
            "ad_groups": [{"id":"m.g","name":"G","campaign":"m.c"}]
        }"#,
        );
        let live = input(
            r#"{
            "customer_id": "1",
            "campaigns": [{"id":"100","name":"C","advertising_channel_type":"SEARCH","campaign_budget":"200"}],
            "ad_groups": [{"id":"300","name":"G","campaign":"100"}],
            "ad_group_claims": {"300": ["keyword_negative"]},
            "claim_labels": {"keyword_negative": "customers/1/labels/777"}
        }"#,
        );
        let report = diff(&declared, &live);

        assert_eq!(report.delete_count, 0);
        assert_eq!(report.claim_plans.len(), 1, "claims: {:?}", report.claim_plans);
        assert_eq!(
            report.claim_plans[0].stale_assoc_rn.as_deref(),
            Some("customers/1/adGroupLabels/300~777")
        );
        assert!(report.adopt_count > 0, "a claim release is pending work");
    }

    #[test]
    fn campaign_level_claim_gates_destroys_and_releases() {
        let declared = input(
            r#"{
            "customer_id": "1",
            "campaigns": [{"id":"m.c","name":"C","advertising_channel_type":"SEARCH","campaign_budget":"m.b"}]
        }"#,
        );
        let live = input(
            r#"{
            "customer_id": "1",
            "campaigns": [{"id":"100","name":"C","advertising_channel_type":"SEARCH","campaign_budget":"200"}],
            "campaign_criteria": [
                {"id":"500","campaign":"100","location":{"geo_target_constant":"geoTargetConstants/2616"}}
            ],
            "campaign_claims": {"100": ["location"]},
            "claim_labels": {"location": "customers/1/labels/779"}
        }"#,
        );
        let report = diff(&declared, &live);

        assert_eq!(
            report.delete_count, 1,
            "the claimed orphaned location should be destroyed: {:?}",
            report.diffs.iter().map(|d| (&d.address, &d.action)).collect::<Vec<_>>()
        );
        let del = report
            .diffs
            .iter()
            .find(|d| matches!(d.action, Action::Delete { .. }))
            .expect("a destroy");
        assert!(del.address.contains("m.c (removed location"));
        assert_eq!(report.claim_plans.len(), 1);
        assert_eq!(
            report.claim_plans[0].stale_assoc_rn.as_deref(),
            Some("customers/1/campaignLabels/100~779")
        );
    }

    #[test]
    fn empty_declared_shared_set_leaves_live_members_alone() {
        // Sets match by bare name, so a declared set with no negative_keywords
        // blocks may be adopting a UI-curated list — never empty it.
        let declared = input(
            r#"{
            "customer_id": "1",
            "shared_sets": [{"id":"m.s","name":"S","type":"NEGATIVE_KEYWORDS"}]
        }"#,
        );
        let live = input(
            r#"{
            "customer_id": "1",
            "shared_sets": [{"id":"600","name":"S","type":"NEGATIVE_KEYWORDS"}],
            "shared_criteria": [
                {"id":"600~1","shared_set":"600","keyword":{"text":"vpn","match_type":"EXACT"}}
            ]
        }"#,
        );
        let report = diff(&declared, &live);

        assert_eq!(
            report.delete_count, 0,
            "live members of an adopted set must not be pruned: {:?}",
            report.diffs.iter().map(|d| (&d.address, &d.action)).collect::<Vec<_>>()
        );
    }
}
