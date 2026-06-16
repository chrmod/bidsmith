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

pub struct DiffReport {
    pub diffs: Vec<ResourceDiff>,
    pub label_plans: Vec<LabelPlanEntry>,
    pub noop_count: usize,
    pub create_count: usize,
    pub update_count: usize,
    pub delete_count: usize,
    /// Resources that match live with no field change but still need a label
    /// written (first-run adoption of an unlabeled resource). Counted so plan /
    /// apply treat label-only work as a pending change rather than a no-op.
    pub adopt_count: usize,
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
/// resource it resolved to (id + its current `bidsmith:address`), or None for a
/// create. Returns None when the resource already carries exactly the right
/// label (a no-op).
fn make_label_plan(
    kind: &'static str,
    address: &str,
    matched: Option<(&str, Option<&str>)>,
    customer_id: &str,
    labels: &HashMap<String, String>,
) -> Option<LabelPlanEntry> {
    if let Some((_, Some(current))) = matched {
        if current == address {
            return None;
        }
    }
    let stale_assoc_rn = match matched {
        Some((live_id, Some(old))) if old != address => labels.get(old).map(|label_rn| {
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
        label_address: address.to_string(),
        existing_label_rn: labels.get(address).cloned(),
        stale_assoc_rn,
    })
}

pub fn diff(declared: &ExportInput, live: &ExportInput) -> DiffReport {
    let mut diffs: Vec<ResourceDiff> = Vec::new();
    let mut label_plans: Vec<LabelPlanEntry> = Vec::new();
    let mut campaign_match: HashMap<String, String> = HashMap::new();
    let mut ad_group_match: HashMap<String, String> = HashMap::new();
    let mut call_asset_match: HashMap<String, String> = HashMap::new();
    let customer_id = declared.customer_id.as_str();

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
        let decl_addr: Vec<&str> = declared.campaigns.iter().map(|c| c.id.as_str()).collect();
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
            if !claimed[li] {
                if let Some(addr) = &l.managed_address {
                    diffs.push(removal_diff("campaign", addr, &l.id));
                }
            }
        }
    }

    // ---- ad_groups (label-first, content-fallback by campaign + name) -----
    let live_ad_groups: Vec<&JsonAdGroup> = live.ad_groups.iter().collect();
    {
        let decl_addr: Vec<&str> = declared.ad_groups.iter().map(|g| g.id.as_str()).collect();
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
            if !claimed[li] {
                if let Some(addr) = &l.managed_address {
                    diffs.push(removal_diff("ad_group", addr, &l.id));
                }
            }
        }
    }

    // ---- ad_group_ads (label hit, else body 1:1; label authorizes destroy) -
    let ad_outcomes =
        match_ad_group_ads(&declared.ad_group_ads, &live.ad_group_ads, &ad_group_match);
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
    for (i, l) in live.ad_group_ads.iter().enumerate() {
        if !ad_claimed[i] {
            if let Some(addr) = &l.managed_address {
                diffs.push(removal_diff("ad_group_ad", addr, &l.id));
            }
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

    let noop_addrs: std::collections::HashSet<&str> = diffs
        .iter()
        .filter(|d| matches!(d.action, Action::NoOp { .. }))
        .map(|d| d.address.as_str())
        .collect();
    let adopt_count = label_plans
        .iter()
        .filter(|p| noop_addrs.contains(p.address.as_str()))
        .count();

    DiffReport {
        diffs,
        label_plans,
        noop_count,
        create_count,
        update_count,
        delete_count,
        adopt_count,
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
        if let Some(&li) = live_by_addr.get(d.id.as_str()) {
            if !consumed[li] && ad_body_key(d) == ad_body_key(&live[li]) {
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
            .entry((l.ad_group.clone(), ad_body_key(l)))
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
                .entry((parent_id.clone(), ad_body_key(d)))
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

    out
}

/// A stable key for an ad's content (everything `diff_ad_group_ad` treats as
/// creation-only). Status is deliberately excluded so identical-bodied ads in
/// different states share a bucket and get assigned 1:1.
fn ad_body_key(a: &JsonAdGroupAd) -> String {
    use std::fmt::Write;
    let mut k = String::new();
    let _ = write!(k, "urls:{}", a.ad.final_urls.join("\u{1f}"));
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

        let actions: Vec<Action> = match_ad_group_ads(&declared, &live, &identity_match())
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

        let actions: Vec<Action> = match_ad_group_ads(&declared, &live, &identity_match())
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

        let actions: Vec<Action> = match_ad_group_ads(&declared, &live, &identity_match())
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

        let actions: Vec<Action> = match_ad_group_ads(&declared, &live, &identity_match())
            .into_iter()
            .map(|(a, _)| a)
            .collect();

        assert!(matches!(&actions[0], Action::Create));
    }

    #[test]
    fn unmapped_ad_group_is_create() {
        let declared = vec![ad("x", "missing_ag", "ENABLED", "Copy")];
        let live = vec![ad("500", "ag", "ENABLED", "Copy")];

        let actions: Vec<Action> = match_ad_group_ads(&declared, &live, &identity_match())
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
}
