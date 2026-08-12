use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value, json};

use crate::api::diff::{Action, DiffReport};
use crate::commands::export::{
    ExportInput, JsonAd, JsonAdGroup, JsonAdGroupAd, JsonAdGroupAsset, JsonAdGroupCriterion,
    JsonBudget, JsonCallAsset, JsonCalloutAsset, JsonCampaign, JsonCampaignAsset,
    JsonAudience, JsonCampaignCriterion, JsonCampaignSharedSet, JsonConversionAction,
    JsonCriterion, JsonCustomAudience, JsonCustomerAsset,
    JsonDemandGenVideoResponsiveAd, JsonResponsiveSearchAd, JsonRsaAsset, JsonSharedSet,
    JsonSitelinkAsset, JsonStructuredSnippetAsset, JsonVideoResponsiveAd, JsonYoutubeVideoAsset,
};

pub struct PlanOperation {
    pub address: String,
    #[allow(dead_code)]
    pub kind: &'static str,
}

pub struct PlanBody {
    pub body: Value,
    pub operations: Vec<PlanOperation>,
    /// Addresses left out of the batch because they reference a custom audience
    /// this run has not created yet — only reachable under `validateOnly`, see
    /// [`build_custom_audience_mutate`].
    pub deferred: Vec<String>,
}

/// A mutation that cannot ride in `GoogleAdsService.Mutate` and needs its own
/// service endpoint.
pub struct ServiceMutation {
    pub endpoint: &'static str,
    pub label: &'static str,
    pub body: Value,
    pub operations: Vec<PlanOperation>,
}

pub struct PlanBuildError {
    pub address: String,
    pub message: String,
}

/// `CustomAudienceOperation` is not a member of `MutateOperation`, so custom
/// audiences go to `CustomAudienceService.MutateCustomAudiences` in a call of
/// their own that has to land before the criteria targeting them (issue #105).
/// That service takes no temp resource names, which is why the create bodies
/// carry none and the batch below waits on the real names this call returns.
pub fn build_custom_audience_mutate(
    input: &ExportInput,
    report: &DiffReport,
    validate_only: bool,
) -> Option<ServiceMutation> {
    let customer_id = input.customer_id.as_str();
    let mut actions: HashMap<&str, &Action> = HashMap::new();
    for d in &report.diffs {
        if d.kind == "custom_audience" {
            actions.insert(d.address.as_str(), &d.action);
        }
    }

    let mut operations: Vec<PlanOperation> = Vec::new();
    let mut ops: Vec<Value> = Vec::new();
    for a in &input.custom_audiences {
        match actions.get(a.id.as_str()) {
            Some(Action::Create) => {
                ops.push(json!({ "create": custom_audience_create(a) }));
            }
            Some(Action::Update { live_id, changed_fields }) => {
                let rn = format!("customers/{customer_id}/customAudiences/{live_id}");
                let fields = crate::api::diff::field_names(changed_fields);
                ops.push(json!({
                    "update": custom_audience_update_body(a, &rn, &fields),
                    "updateMask": fields.join(","),
                }));
            }
            _ => continue,
        }
        operations.push(PlanOperation { address: a.id.clone(), kind: "custom_audience" });
    }
    if ops.is_empty() {
        return None;
    }
    Some(ServiceMutation {
        endpoint: "customAudiences:mutate",
        label: "custom audiences",
        body: json!({ "operations": ops, "validateOnly": validate_only }),
        operations,
    })
}

pub fn build_mutate_with_diff(
    input: &ExportInput,
    report: &DiffReport,
    validate_only: bool,
    created_custom_audiences: &HashMap<String, String>,
) -> Result<PlanBody, Vec<PlanBuildError>> {
    let customer_id = input.customer_id.as_str();
    let mut refs: HashMap<String, String> = HashMap::new();
    let mut next: i32 = -1;
    let mut create_set: HashSet<String> = HashSet::new();

    let mut update_set: HashMap<String, Vec<String>> = HashMap::new();

    // Build resource_name refs for every declared address: matched (NoOp or
    // Update) resources get the real live resource_name; Create resources
    // get a temp negative id (composite where the API demands a parent~child
    // format).
    for d in &report.diffs {
        let segment = match d.kind {
            "campaign_budget" => "campaignBudgets",
            "campaign" => "campaigns",
            "ad_group" => "adGroups",
            "ad_group_ad" => "adGroupAds",
            "ad_group_criterion" => "adGroupCriteria",
            "campaign_criterion" => "campaignCriteria",
            "conversion_action" => "conversionActions",
            "call_asset" => "assets",
            "sitelink_asset" => "assets",
            "callout_asset" => "assets",
            "structured_snippet_asset" => "assets",
            "youtube_video_asset" => "assets",
            "customer_asset" => "customerAssets",
            "campaign_asset" => "campaignAssets",
            "ad_group_asset" => "adGroupAssets",
            "shared_set" => "sharedSets",
            "shared_criterion" => "sharedCriteria",
            "campaign_shared_set" => "campaignSharedSets",
            "custom_audience" => "customAudiences",
            _ => continue,
        };
        if let Some(live_id) = d.action.live_id() {
            refs.insert(
                d.address.clone(),
                format!("customers/{customer_id}/{segment}/{live_id}"),
            );
            if let Action::Update { changed_fields, .. } = &d.action {
                update_set.insert(d.address.clone(), crate::api::diff::field_names(changed_fields));
            }
        } else {
            create_set.insert(d.address.clone());
            // A custom audience is created by its own service before this batch
            // is built; it has a real resource name or none at all (a
            // `validateOnly` run creates nothing), never a temp id.
            if d.kind == "custom_audience" {
                if let Some(rn) = created_custom_audiences.get(&d.address) {
                    refs.insert(d.address.clone(), rn.clone());
                }
                continue;
            }
            let rn = match d.kind {
                "ad_group_ad" => {
                    let parent_addr = input
                        .ad_group_ads
                        .iter()
                        .find(|a| a.id == d.address)
                        .map(|a| a.ad_group.as_str())
                        .unwrap_or("");
                    composite_rn(customer_id, segment, &refs, parent_addr, next)
                }
                "ad_group_criterion" => {
                    let parent_addr = input
                        .ad_group_criteria
                        .iter()
                        .find(|c| c.id == d.address)
                        .map(|c| c.ad_group.as_str())
                        .unwrap_or("");
                    composite_rn(customer_id, segment, &refs, parent_addr, next)
                }
                "campaign_shared_set" => {
                    let css = input
                        .campaign_shared_sets
                        .iter()
                        .find(|c| c.id == d.address);
                    let parent_addr = css.map(|c| c.campaign.as_str()).unwrap_or("");
                    let parent_id = if parent_addr.starts_with("customers/") {
                        parent_addr.rsplit('/').next().unwrap_or("0").to_string()
                    } else {
                        refs.get(parent_addr)
                            .and_then(|rn| rn.rsplit('/').next())
                            .unwrap_or("0")
                            .to_string()
                    };
                    let set_addr = css.map(|c| c.shared_set.as_str()).unwrap_or("");
                    let set_id = if set_addr.starts_with("customers/") {
                        set_addr.rsplit('/').next().unwrap_or("0").to_string()
                    } else {
                        refs.get(set_addr)
                            .and_then(|rn| rn.rsplit('/').next())
                            .unwrap_or("0")
                            .to_string()
                    };
                    format!(
                        "customers/{customer_id}/{segment}/{parent_id}~{set_id}"
                    )
                }
                "shared_criterion" => {
                    let sc = input
                        .shared_criteria
                        .iter()
                        .find(|c| c.id == d.address);
                    let set_addr = sc.map(|c| c.shared_set.as_str()).unwrap_or("");
                    let set_id = if set_addr.starts_with("customers/") {
                        set_addr.rsplit('/').next().unwrap_or("0").to_string()
                    } else {
                        refs.get(set_addr)
                            .and_then(|rn| rn.rsplit('/').next())
                            .unwrap_or("0")
                            .to_string()
                    };
                    format!(
                        "customers/{customer_id}/{segment}/{set_id}~{next}"
                    )
                }
                "campaign_asset" => {
                    let ca = input.campaign_assets.iter().find(|c| c.id == d.address);
                    let parent_id = ca
                        .map(|c| id_from_ref(&refs, &c.campaign))
                        .unwrap_or_else(|| "0".to_string());
                    let asset_id = ca
                        .map(|c| id_from_ref(&refs, &c.asset))
                        .unwrap_or_else(|| "0".to_string());
                    let field_type = ca.map(|c| c.field_type.as_str()).unwrap_or("");
                    format!(
                        "customers/{customer_id}/{segment}/{parent_id}~{asset_id}~{field_type}"
                    )
                }
                "ad_group_asset" => {
                    let ga = input.ad_group_assets.iter().find(|c| c.id == d.address);
                    let parent_id = ga
                        .map(|c| id_from_ref(&refs, &c.ad_group))
                        .unwrap_or_else(|| "0".to_string());
                    let asset_id = ga
                        .map(|c| id_from_ref(&refs, &c.asset))
                        .unwrap_or_else(|| "0".to_string());
                    let field_type = ga.map(|c| c.field_type.as_str()).unwrap_or("");
                    format!(
                        "customers/{customer_id}/{segment}/{parent_id}~{asset_id}~{field_type}"
                    )
                }
                _ => temp_rn(customer_id, segment, next),
            };
            refs.insert(d.address.clone(), rn);
            next -= 1;
        }
    }

    // Now emit CREATE operations only for resources marked Create.
    let mut errors: Vec<PlanBuildError> = Vec::new();
    let mut operations: Vec<PlanOperation> = Vec::new();
    let mut mutate_ops: Vec<Value> = Vec::new();
    let mut deferred: Vec<String> = Vec::new();

    // Removes go first, before any create. Replacing an immutable resource
    // (an RSA copy edit) is a destroy of the old body plus a create of the new
    // one; the API applies a single atomic mutate's ops in array order and
    // checks per-parent caps against that running state, so creating ahead of
    // destroying transiently exceeds the 3-enabled-RSAs-per-ad-group cap and
    // sinks the whole batch. Destroying first keeps the running count within the
    // cap. Removes reference live resource_names only (no temp-id dependency on
    // a create), so leading with them is order-safe. Sorted child-first so a
    // campaign and one of its ad groups can be destroyed in the same batch.
    let mut deletes: Vec<(&'static str, String, String)> = Vec::new();
    for d in &report.diffs {
        if let Action::Delete { live_id } = &d.action {
            let Some(segment) = remove_segment_for(d.kind) else {
                continue;
            };
            deletes.push((
                d.kind,
                format!("customers/{customer_id}/{segment}/{live_id}"),
                d.address.clone(),
            ));
        }
    }
    deletes.sort_by_key(|(kind, _, _)| removal_order_index(kind));
    for (kind, rn, address) in deletes {
        if let Some(env) = remove_envelope_for(kind) {
            mutate_ops.push(json!({ env: { "remove": rn } }));
            operations.push(PlanOperation { address, kind });
        }
    }

    for b in &input.campaign_budgets {
        let Some(rn) = plan_rn(&refs, &b.id, "campaign_budget", &mut errors) else {
            continue;
        };
        if create_set.contains(&b.id) {
            mutate_ops.push(json!({
                "campaignBudgetOperation": { "create": budget_create(b, rn) }
            }));
            operations.push(PlanOperation { address: b.id.clone(), kind: "campaign_budget" });
        } else if let Some(fields) = update_set.get(&b.id) {
            mutate_ops.push(json!({
                "campaignBudgetOperation": {
                    "update": budget_update_body(b, rn, fields),
                    "updateMask": fields.join(","),
                }
            }));
            operations.push(PlanOperation { address: b.id.clone(), kind: "campaign_budget" });
        }
    }
    for c in &input.campaigns {
        let Some(rn) = plan_rn(&refs, &c.id, "campaign", &mut errors) else {
            continue;
        };
        if create_set.contains(&c.id) {
            let budget_rn = match resolve(&refs, &c.campaign_budget, &c.id, "campaign_budget", &mut errors) {
                Some(s) => s,
                None => continue,
            };
            mutate_ops.push(json!({
                "campaignOperation": { "create": campaign_create(c, rn, &budget_rn) }
            }));
            operations.push(PlanOperation { address: c.id.clone(), kind: "campaign" });
        } else if let Some(fields) = update_set.get(&c.id) {
            mutate_ops.push(json!({
                "campaignOperation": {
                    "update": campaign_update_body(c, rn, fields),
                    "updateMask": campaign_update_mask(fields),
                }
            }));
            operations.push(PlanOperation { address: c.id.clone(), kind: "campaign" });
        }
    }
    for g in &input.ad_groups {
        let Some(rn) = plan_rn(&refs, &g.id, "ad_group", &mut errors) else {
            continue;
        };
        if create_set.contains(&g.id) {
            let campaign_rn = match resolve(&refs, &g.campaign, &g.id, "campaign", &mut errors) {
                Some(s) => s,
                None => continue,
            };
            mutate_ops.push(json!({
                "adGroupOperation": { "create": ad_group_create(g, rn, &campaign_rn) }
            }));
            operations.push(PlanOperation { address: g.id.clone(), kind: "ad_group" });
        } else if let Some(fields) = update_set.get(&g.id) {
            mutate_ops.push(json!({
                "adGroupOperation": {
                    "update": ad_group_update_body(g, rn, fields),
                    "updateMask": fields.join(","),
                }
            }));
            operations.push(PlanOperation { address: g.id.clone(), kind: "ad_group" });
        }
    }
    // Ahead of the ads that reference them: the API resolves a temp resource
    // name only against an operation earlier in the same list.
    for a in &input.youtube_video_assets {
        let Some(rn) = plan_rn(&refs, &a.id, "youtube_video_asset", &mut errors) else {
            continue;
        };
        if create_set.contains(&a.id) {
            mutate_ops.push(json!({
                "assetOperation": { "create": youtube_video_asset_create(a, rn) }
            }));
            operations.push(PlanOperation { address: a.id.clone(), kind: "youtube_video_asset" });
        }
    }
    for a in &input.ad_group_ads {
        let Some(rn) = plan_rn(&refs, &a.id, "ad_group_ad", &mut errors) else {
            continue;
        };
        if create_set.contains(&a.id) {
            let ag_rn = match resolve(&refs, &a.ad_group, &a.id, "ad_group", &mut errors) {
                Some(s) => s,
                None => continue,
            };
            let videos = match resolve_ad_videos(&refs, a, &mut errors) {
                Some(v) => v,
                None => continue,
            };
            mutate_ops.push(json!({
                "adGroupAdOperation": { "create": ad_group_ad_create(a, rn, &ag_rn, &videos) }
            }));
            operations.push(PlanOperation { address: a.id.clone(), kind: "ad_group_ad" });
        } else if let Some(fields) = update_set.get(&a.id) {
            mutate_ops.push(json!({
                "adGroupAdOperation": {
                    "update": ad_group_ad_update_body(a, rn, fields),
                    "updateMask": fields.join(","),
                }
            }));
            operations.push(PlanOperation { address: a.id.clone(), kind: "ad_group_ad" });
        }
    }
    for cr in &input.ad_group_criteria {
        let Some(rn) = plan_rn(&refs, &cr.id, "ad_group_criterion", &mut errors) else {
            continue;
        };
        if create_set.contains(&cr.id) {
            let ag_rn = match resolve(&refs, &cr.ad_group, &cr.id, "ad_group", &mut errors) {
                Some(s) => s,
                None => continue,
            };
            let audience_rn =
                match criterion_audience_rn(&cr.target, &refs, &create_set, &cr.id, &mut errors) {
                    AudienceRef::Ready(rn) => rn,
                    AudienceRef::Pending => {
                        deferred.push(cr.id.clone());
                        continue;
                    }
                    AudienceRef::Unresolvable => continue,
                };
            mutate_ops.push(json!({
                "adGroupCriterionOperation": {
                    "create": ad_group_criterion_create(cr, rn, &ag_rn, audience_rn)
                }
            }));
            operations.push(PlanOperation { address: cr.id.clone(), kind: "ad_group_criterion" });
        } else if let Some(fields) = update_set.get(&cr.id) {
            mutate_ops.push(json!({
                "adGroupCriterionOperation": {
                    "update": ad_group_criterion_update_body(cr, rn, fields),
                    "updateMask": fields.join(","),
                }
            }));
            operations.push(PlanOperation { address: cr.id.clone(), kind: "ad_group_criterion" });
        }
    }
    for cr in &input.campaign_criteria {
        let Some(rn) = plan_rn(&refs, &cr.id, "campaign_criterion", &mut errors) else {
            continue;
        };
        if create_set.contains(&cr.id) {
            let camp_rn = match resolve(&refs, &cr.campaign, &cr.id, "campaign", &mut errors) {
                Some(s) => s,
                None => continue,
            };
            let audience_rn =
                match criterion_audience_rn(&cr.target, &refs, &create_set, &cr.id, &mut errors) {
                    AudienceRef::Ready(rn) => rn,
                    AudienceRef::Pending => {
                        deferred.push(cr.id.clone());
                        continue;
                    }
                    AudienceRef::Unresolvable => continue,
                };
            mutate_ops.push(json!({
                "campaignCriterionOperation": {
                    "create": campaign_criterion_create(cr, &camp_rn, audience_rn)
                }
            }));
            operations.push(PlanOperation { address: cr.id.clone(), kind: "campaign_criterion" });
        } else if let Some(fields) = update_set.get(&cr.id) {
            mutate_ops.push(json!({
                "campaignCriterionOperation": {
                    "update": campaign_criterion_update_body(cr, rn, fields),
                    "updateMask": fields.join(","),
                }
            }));
            operations.push(PlanOperation { address: cr.id.clone(), kind: "campaign_criterion" });
        }
    }
    for c in &input.conversion_actions {
        let Some(rn) = plan_rn(&refs, &c.id, "conversion_action", &mut errors) else {
            continue;
        };
        if create_set.contains(&c.id) {
            mutate_ops.push(json!({
                "conversionActionOperation": { "create": conversion_action_create(c, rn) }
            }));
            operations
                .push(PlanOperation { address: c.id.clone(), kind: "conversion_action" });
        } else if let Some(fields) = update_set.get(&c.id) {
            mutate_ops.push(json!({
                "conversionActionOperation": {
                    "update": conversion_action_update_body(c, rn, fields),
                    "updateMask": fields.join(","),
                }
            }));
            operations
                .push(PlanOperation { address: c.id.clone(), kind: "conversion_action" });
        }
    }
    for a in &input.call_assets {
        let Some(rn) = plan_rn(&refs, &a.id, "call_asset", &mut errors) else {
            continue;
        };
        if create_set.contains(&a.id) {
            let action_rn = match a.call_conversion_action.as_ref() {
                Some(addr) if addr.starts_with("customers/") => Some(addr.clone()),
                Some(addr) => match resolve(&refs, addr, &a.id, "call_conversion_action", &mut errors) {
                    Some(s) => Some(s),
                    None => continue,
                },
                None => None,
            };
            mutate_ops.push(json!({
                "assetOperation": { "create": call_asset_create(a, rn, action_rn.as_deref()) }
            }));
            operations.push(PlanOperation { address: a.id.clone(), kind: "call_asset" });
        }
    }
    for a in &input.sitelink_assets {
        let Some(rn) = plan_rn(&refs, &a.id, "sitelink_asset", &mut errors) else {
            continue;
        };
        if create_set.contains(&a.id) {
            mutate_ops.push(json!({
                "assetOperation": { "create": sitelink_asset_create(a, rn) }
            }));
            operations.push(PlanOperation { address: a.id.clone(), kind: "sitelink_asset" });
        }
    }
    for a in &input.callout_assets {
        let Some(rn) = plan_rn(&refs, &a.id, "callout_asset", &mut errors) else {
            continue;
        };
        if create_set.contains(&a.id) {
            mutate_ops.push(json!({
                "assetOperation": { "create": callout_asset_create(a, rn) }
            }));
            operations.push(PlanOperation { address: a.id.clone(), kind: "callout_asset" });
        }
    }
    for a in &input.structured_snippet_assets {
        let Some(rn) = plan_rn(&refs, &a.id, "structured_snippet_asset", &mut errors) else {
            continue;
        };
        if create_set.contains(&a.id) {
            mutate_ops.push(json!({
                "assetOperation": { "create": structured_snippet_asset_create(a, rn) }
            }));
            operations.push(PlanOperation {
                address: a.id.clone(),
                kind: "structured_snippet_asset",
            });
        }
    }
    for a in &input.customer_assets {
        let Some(rn) = plan_rn(&refs, &a.id, "customer_asset", &mut errors) else {
            continue;
        };
        if create_set.contains(&a.id) {
            let asset_rn = match resolve(&refs, &a.asset, &a.id, "asset", &mut errors) {
                Some(s) => s,
                None => continue,
            };
            mutate_ops.push(json!({
                "customerAssetOperation": { "create": customer_asset_create(a, rn, &asset_rn) }
            }));
            operations.push(PlanOperation { address: a.id.clone(), kind: "customer_asset" });
        } else if let Some(fields) = update_set.get(&a.id) {
            mutate_ops.push(json!({
                "customerAssetOperation": {
                    "update": customer_asset_update_body(a, rn, fields),
                    "updateMask": fields.join(","),
                }
            }));
            operations.push(PlanOperation { address: a.id.clone(), kind: "customer_asset" });
        }
    }
    for a in &input.campaign_assets {
        let Some(rn) = plan_rn(&refs, &a.id, "campaign_asset", &mut errors) else {
            continue;
        };
        if create_set.contains(&a.id) {
            let campaign_rn = match resolve_ref_or_literal(&refs, &a.campaign, &a.id, "campaign", &mut errors) {
                Some(s) => s,
                None => continue,
            };
            let asset_rn = match resolve(&refs, &a.asset, &a.id, "asset", &mut errors) {
                Some(s) => s,
                None => continue,
            };
            mutate_ops.push(json!({
                "campaignAssetOperation": {
                    "create": campaign_asset_create(a, &campaign_rn, &asset_rn)
                }
            }));
            operations.push(PlanOperation { address: a.id.clone(), kind: "campaign_asset" });
        } else if let Some(fields) = update_set.get(&a.id) {
            mutate_ops.push(json!({
                "campaignAssetOperation": {
                    "update": campaign_asset_update_body(a, rn, fields),
                    "updateMask": fields.join(","),
                }
            }));
            operations.push(PlanOperation { address: a.id.clone(), kind: "campaign_asset" });
        }
    }
    for a in &input.ad_group_assets {
        let Some(rn) = plan_rn(&refs, &a.id, "ad_group_asset", &mut errors) else {
            continue;
        };
        if create_set.contains(&a.id) {
            let ad_group_rn = match resolve_ref_or_literal(&refs, &a.ad_group, &a.id, "ad_group", &mut errors) {
                Some(s) => s,
                None => continue,
            };
            let asset_rn = match resolve(&refs, &a.asset, &a.id, "asset", &mut errors) {
                Some(s) => s,
                None => continue,
            };
            mutate_ops.push(json!({
                "adGroupAssetOperation": {
                    "create": ad_group_asset_create(a, &ad_group_rn, &asset_rn)
                }
            }));
            operations.push(PlanOperation { address: a.id.clone(), kind: "ad_group_asset" });
        } else if let Some(fields) = update_set.get(&a.id) {
            mutate_ops.push(json!({
                "adGroupAssetOperation": {
                    "update": ad_group_asset_update_body(a, rn, fields),
                    "updateMask": fields.join(","),
                }
            }));
            operations.push(PlanOperation { address: a.id.clone(), kind: "ad_group_asset" });
        }
    }

    for s in &input.shared_sets {
        let Some(rn) = plan_rn(&refs, &s.id, "shared_set", &mut errors) else {
            continue;
        };
        if create_set.contains(&s.id) {
            mutate_ops.push(json!({
                "sharedSetOperation": { "create": shared_set_create(s, rn) }
            }));
            operations.push(PlanOperation { address: s.id.clone(), kind: "shared_set" });
        } else if let Some(fields) = update_set.get(&s.id) {
            mutate_ops.push(json!({
                "sharedSetOperation": {
                    "update": shared_set_update_body(s, rn, fields),
                    "updateMask": fields.join(","),
                }
            }));
            operations.push(PlanOperation { address: s.id.clone(), kind: "shared_set" });
        }
    }

    for c in &input.shared_criteria {
        if !create_set.contains(&c.id) {
            continue;
        }
        let set_rn = if c.shared_set.starts_with("customers/") {
            c.shared_set.clone()
        } else {
            match resolve(&refs, &c.shared_set, &c.id, "shared_set", &mut errors) {
                Some(s) => s,
                None => continue,
            }
        };
        mutate_ops.push(json!({
            "sharedCriterionOperation": {
                "create": shared_criterion_create(&set_rn, &c.keyword)
            }
        }));
        operations.push(PlanOperation {
            address: c.id.clone(),
            kind: "shared_criterion",
        });
    }

    for cs in &input.campaign_shared_sets {
        let Some(rn) = plan_rn(&refs, &cs.id, "campaign_shared_set", &mut errors) else {
            continue;
        };
        if create_set.contains(&cs.id) {
            let camp_rn = if cs.campaign.starts_with("customers/") {
                cs.campaign.clone()
            } else {
                match resolve(&refs, &cs.campaign, &cs.id, "campaign", &mut errors) {
                    Some(s) => s,
                    None => continue,
                }
            };
            let set_rn = if cs.shared_set.starts_with("customers/") {
                cs.shared_set.clone()
            } else {
                match resolve(&refs, &cs.shared_set, &cs.id, "shared_set", &mut errors) {
                    Some(s) => s,
                    None => continue,
                }
            };
            mutate_ops.push(json!({
                "campaignSharedSetOperation": {
                    "create": campaign_shared_set_create(cs, &camp_rn, &set_rn)
                }
            }));
            operations.push(PlanOperation {
                address: cs.id.clone(),
                kind: "campaign_shared_set",
            });
        } else if let Some(fields) = update_set.get(&cs.id) {
            mutate_ops.push(json!({
                "campaignSharedSetOperation": {
                    "update": campaign_shared_set_update_body(cs, rn, fields),
                    "updateMask": fields.join(","),
                }
            }));
            operations.push(PlanOperation {
                address: cs.id.clone(),
                kind: "campaign_shared_set",
            });
        }
    }

    // Label writes: ensure each created / adopted / relabeled resource carries
    // its bidsmith:address label. A brand-new label gets a temp id; an existing
    // one is reused by resource_name (a duplicate label name is an API error).
    // Emitted after the resource creates so the association can reference the
    // resource's own (temp or live) resource_name. A stale association from a
    // prior address is removed. New labels continue the shared `next` temp-id
    // counter: temp ids are unique per request across *all* resource types, so a
    // label restarting at -1 would collide with the first created resource.
    for plan in &report.label_plans {
        let Some(resource_rn) = refs.get(&plan.address).cloned() else {
            errors.push(PlanBuildError {
                address: plan.address.clone(),
                message: "internal error: no resource name for label target".to_string(),
            });
            continue;
        };
        let label_rn = match &plan.existing_label_rn {
            Some(rn) => rn.clone(),
            None => {
                let rn = format!("customers/{customer_id}/labels/{next}");
                next -= 1;
                mutate_ops.push(json!({
                    "labelOperation": {
                        "create": {
                            "resourceName": rn,
                            "name": format!(
                                "{}{}",
                                crate::commands::export::ADDRESS_LABEL_PREFIX,
                                plan.label_address
                            ),
                        }
                    }
                }));
                operations.push(PlanOperation { address: plan.address.clone(), kind: "label" });
                rn
            }
        };
        let (assoc_env, entity_field) = label_assoc_op(plan.kind);
        let mut assoc = Map::new();
        assoc.insert(entity_field.into(), Value::String(resource_rn));
        assoc.insert("label".into(), Value::String(label_rn));
        mutate_ops.push(json!({ assoc_env: { "create": Value::Object(assoc) } }));
        operations.push(PlanOperation { address: plan.address.clone(), kind: "label" });

        if let Some(stale) = &plan.stale_assoc_rn {
            mutate_ops.push(json!({ assoc_env: { "remove": stale } }));
            operations.push(PlanOperation { address: plan.address.clone(), kind: "label" });
        }
    }

    // Claim writes: `bidsmith:owns=<category>` associations recording which
    // criterion categories bidsmith manages on each campaign / ad group.
    // Several parents can claim the same category in one batch, so a
    // freshly-created claim label is minted once and its temp resource_name
    // shared. Releases (stale_assoc_rn) reference live resource_names only.
    let mut claim_label_rns: HashMap<&str, String> = HashMap::new();
    for plan in &report.claim_plans {
        if let Some(stale) = &plan.stale_assoc_rn {
            let (assoc_env, _) = label_assoc_op(plan.kind);
            mutate_ops.push(json!({ assoc_env: { "remove": stale } }));
            operations.push(PlanOperation { address: plan.address.clone(), kind: "label" });
            continue;
        }
        let Some(parent_rn) = refs.get(&plan.address).cloned() else {
            errors.push(PlanBuildError {
                address: plan.address.clone(),
                message: "internal error: no resource name for claim target".to_string(),
            });
            continue;
        };
        let label_rn = match plan
            .existing_label_rn
            .clone()
            .or_else(|| claim_label_rns.get(plan.category).cloned())
        {
            Some(rn) => rn,
            None => {
                let rn = format!("customers/{customer_id}/labels/{next}");
                next -= 1;
                mutate_ops.push(json!({
                    "labelOperation": {
                        "create": {
                            "resourceName": rn,
                            "name": format!(
                                "{}{}",
                                crate::commands::export::OWNS_LABEL_PREFIX,
                                plan.category
                            ),
                        }
                    }
                }));
                operations.push(PlanOperation { address: plan.address.clone(), kind: "label" });
                claim_label_rns.insert(plan.category, rn.clone());
                rn
            }
        };
        let (assoc_env, entity_field) = label_assoc_op(plan.kind);
        let mut assoc = Map::new();
        assoc.insert(entity_field.into(), Value::String(parent_rn));
        assoc.insert("label".into(), Value::String(label_rn));
        mutate_ops.push(json!({ assoc_env: { "create": Value::Object(assoc) } }));
        operations.push(PlanOperation { address: plan.address.clone(), kind: "label" });
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    let body = json!({
        "mutateOperations": mutate_ops,
        "validateOnly": validate_only,
    });
    Ok(PlanBody { body, operations, deferred })
}

fn remove_segment_for(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "campaign_budget" => "campaignBudgets",
        "campaign" => "campaigns",
        "ad_group" => "adGroups",
        "ad_group_ad" => "adGroupAds",
        "ad_group_criterion" => "adGroupCriteria",
        "campaign_criterion" => "campaignCriteria",
        "shared_criterion" => "sharedCriteria",
        _ => return None,
    })
}

/// Build a REMOVE-only googleAds:mutate body for the given (kind, resource_name)
/// pairs. Sorts into the API's required removal order (children before parents)
/// before emitting operations. Unknown kinds are skipped.
pub fn build_remove_only_mutate(operations: &[(&str, String)], validate_only: bool) -> Value {
    let mut ordered: Vec<&(&str, String)> = operations.iter().collect();
    ordered.sort_by_key(|(kind, _)| removal_order_index(kind));
    let mutate_ops: Vec<Value> = ordered
        .iter()
        .filter_map(|(kind, rn)| {
            remove_envelope_for(kind).map(|env| json!({ env: { "remove": rn } }))
        })
        .collect();
    json!({
        "mutateOperations": mutate_ops,
        "validateOnly": validate_only,
    })
}

/// The mutate envelope and entity-reference field for a label association of a
/// given labelable kind.
fn label_assoc_op(kind: &str) -> (&'static str, &'static str) {
    match kind {
        "ad_group" => ("adGroupLabelOperation", "adGroup"),
        "ad_group_ad" => ("adGroupAdLabelOperation", "adGroupAd"),
        _ => ("campaignLabelOperation", "campaign"),
    }
}

fn remove_envelope_for(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "campaign_budget" => "campaignBudgetOperation",
        "campaign" => "campaignOperation",
        "ad_group" => "adGroupOperation",
        "ad_group_ad" => "adGroupAdOperation",
        "ad_group_criterion" => "adGroupCriterionOperation",
        "campaign_criterion" => "campaignCriterionOperation",
        "shared_criterion" => "sharedCriterionOperation",
        _ => return None,
    })
}

fn removal_order_index(kind: &str) -> usize {
    match kind {
        "ad_group_criterion" => 0,
        "campaign_criterion" => 1,
        "ad_group_ad" => 2,
        "ad_group" => 3,
        "campaign" => 4,
        "campaign_budget" => 5,
        _ => usize::MAX,
    }
}

fn to_micro_degrees(decimal_degrees: f64) -> i64 {
    (decimal_degrees * 1_000_000.0).round() as i64
}

fn temp_rn(customer_id: &str, segment: &str, idx: i32) -> String {
    format!("customers/{customer_id}/{segment}/{idx}")
}

fn composite_rn(
    customer_id: &str,
    segment: &str,
    refs: &HashMap<String, String>,
    parent_address: &str,
    idx: i32,
) -> String {
    let parent_id = refs
        .get(parent_address)
        .and_then(|rn| rn.rsplit('/').next())
        .unwrap_or("0");
    format!("customers/{customer_id}/{segment}/{parent_id}~{idx}")
}

fn budget_update_body(b: &JsonBudget, resource_name: &str, fields: &[String]) -> Value {
    let mut m = Map::new();
    m.insert("resourceName".into(), Value::String(resource_name.to_string()));
    for f in fields {
        match f.as_str() {
            "name" => {
                m.insert("name".into(), Value::String(b.name.clone()));
            }
            "amount_micros" => {
                m.insert(
                    "amountMicros".into(),
                    Value::String(b.amount_micros.to_string()),
                );
            }
            "delivery_method" => {
                m.insert(
                    "deliveryMethod".into(),
                    b.delivery_method
                        .clone()
                        .map(Value::String)
                        .unwrap_or(Value::Null),
                );
            }
            "explicitly_shared" => {
                m.insert(
                    "explicitlyShared".into(),
                    b.explicitly_shared.map(Value::Bool).unwrap_or(Value::Null),
                );
            }
            _ => {}
        }
    }
    Value::Object(m)
}

/// A changed field is its own mask path, except a bidding-strategy switch —
/// which the API only accepts spelled out as the strategy's subfields, see
/// [`crate::schema::CAMPAIGN_BIDDING_MASK_PATHS`].
fn campaign_update_mask(fields: &[String]) -> String {
    let mut paths: Vec<&str> = Vec::new();
    for f in fields {
        match crate::schema::campaign_bidding_mask_paths(f) {
            Some(strategy) => paths.extend_from_slice(strategy),
            None => paths.push(f.as_str()),
        }
    }
    paths.join(",")
}

fn campaign_update_body(c: &JsonCampaign, resource_name: &str, fields: &[String]) -> Value {
    let mut m = Map::new();
    m.insert("resourceName".into(), Value::String(resource_name.to_string()));
    let mut manual_cpc_sub: Option<Map<String, Value>> = None;
    let mut network_sub: Option<Map<String, Value>> = None;
    let mut geo_sub: Option<Map<String, Value>> = None;
    for f in fields {
        match f.as_str() {
            "name" => {
                m.insert("name".into(), Value::String(c.name.clone()));
            }
            "status" => {
                if let Some(s) = &c.status {
                    m.insert("status".into(), Value::String(s.clone()));
                }
            }
            "contains_eu_political_advertising" => {
                if let Some(s) = &c.contains_eu_political_advertising {
                    m.insert(
                        "containsEuPoliticalAdvertising".into(),
                        Value::String(s.clone()),
                    );
                }
            }
            "start_date" => {
                if let Some(s) = &c.start_date {
                    m.insert("startDate".into(), Value::String(s.clone()));
                }
            }
            "end_date" => {
                if let Some(s) = &c.end_date {
                    m.insert("endDate".into(), Value::String(s.clone()));
                }
            }
            "manual_cpc.enhanced_cpc_enabled" => {
                let sub = manual_cpc_sub.get_or_insert_with(Map::new);
                if let Some(e) = c.manual_cpc.as_ref().and_then(|m| m.enhanced_cpc_enabled) {
                    sub.insert("enhancedCpcEnabled".into(), Value::Bool(e));
                }
            }
            // Switching which member of the bidding `oneof` is set.
            "manual_cpc" | "manual_cpm" | "manual_cpv" | "target_cpm" | "target_cpv" => {
                if let Some((field, body)) = bidding_strategy_value(c) {
                    m.insert(field.into(), body);
                }
            }
            "network_settings.target_google_search" => {
                let sub = network_sub.get_or_insert_with(Map::new);
                if let Some(v) = c
                    .network_settings
                    .as_ref()
                    .and_then(|n| n.target_google_search)
                {
                    sub.insert("targetGoogleSearch".into(), Value::Bool(v));
                }
            }
            "network_settings.target_search_network" => {
                let sub = network_sub.get_or_insert_with(Map::new);
                if let Some(v) = c
                    .network_settings
                    .as_ref()
                    .and_then(|n| n.target_search_network)
                {
                    sub.insert("targetSearchNetwork".into(), Value::Bool(v));
                }
            }
            "network_settings.target_content_network" => {
                let sub = network_sub.get_or_insert_with(Map::new);
                if let Some(v) = c
                    .network_settings
                    .as_ref()
                    .and_then(|n| n.target_content_network)
                {
                    sub.insert("targetContentNetwork".into(), Value::Bool(v));
                }
            }
            "network_settings.target_partner_search_network" => {
                let sub = network_sub.get_or_insert_with(Map::new);
                if let Some(v) = c
                    .network_settings
                    .as_ref()
                    .and_then(|n| n.target_partner_search_network)
                {
                    sub.insert("targetPartnerSearchNetwork".into(), Value::Bool(v));
                }
            }
            // A repeated field is replaced wholesale; an empty list clears it.
            "frequency_caps" => {
                m.insert("frequencyCaps".into(), frequency_caps_value(c));
            }
            other => {
                if let Some((field, json)) = other
                    .strip_prefix("geo_target_type_setting.")
                    .and_then(|f| {
                        crate::schema::GEO_TARGET_TYPE_FIELDS
                            .iter()
                            .find(|(field, _)| *field == f)
                    })
                {
                    let sub = geo_sub.get_or_insert_with(Map::new);
                    if let Some(v) = c.geo_target_type_setting.as_ref().and_then(|g| g.get(field)) {
                        sub.insert((*json).into(), Value::String(v.to_string()));
                    }
                }
            }
        }
    }
    if let Some(sub) = manual_cpc_sub {
        m.insert("manualCpc".into(), Value::Object(sub));
    }
    if let Some(sub) = network_sub {
        m.insert("networkSettings".into(), Value::Object(sub));
    }
    if let Some(sub) = geo_sub {
        m.insert("geoTargetTypeSetting".into(), Value::Object(sub));
    }
    Value::Object(m)
}

fn ad_group_update_body(g: &JsonAdGroup, resource_name: &str, fields: &[String]) -> Value {
    let mut m = Map::new();
    m.insert("resourceName".into(), Value::String(resource_name.to_string()));
    for f in fields {
        match f.as_str() {
            "name" => {
                m.insert("name".into(), Value::String(g.name.clone()));
            }
            "status" => {
                if let Some(s) = &g.status {
                    m.insert("status".into(), Value::String(s.clone()));
                }
            }
            "type" => {
                if let Some(t) = &g.ty {
                    m.insert("type".into(), Value::String(t.clone()));
                }
            }
            other => {
                if let Some((field, json)) = crate::schema::AD_GROUP_BID_FIELDS
                    .iter()
                    .find(|(field, _)| *field == other)
                {
                    if let Some(c) = g.bid(field) {
                        m.insert((*json).into(), Value::String(c.to_string()));
                    }
                }
            }
        }
    }
    Value::Object(m)
}

fn ad_group_ad_update_body(a: &JsonAdGroupAd, resource_name: &str, fields: &[String]) -> Value {
    let mut m = Map::new();
    m.insert("resourceName".into(), Value::String(resource_name.to_string()));
    for f in fields {
        if f == "status" {
            if let Some(s) = &a.status {
                m.insert("status".into(), Value::String(s.clone()));
            }
        }
    }
    Value::Object(m)
}

fn ad_group_criterion_update_body(
    cr: &JsonAdGroupCriterion,
    resource_name: &str,
    fields: &[String],
) -> Value {
    let mut m = Map::new();
    m.insert("resourceName".into(), Value::String(resource_name.to_string()));
    for f in fields {
        match f.as_str() {
            "status" => {
                if let Some(s) = &cr.status {
                    m.insert("status".into(), Value::String(s.clone()));
                }
            }
            "negative" => {
                if let Some(n) = cr.negative {
                    m.insert("negative".into(), Value::Bool(n));
                }
            }
            "cpc_bid_micros" => {
                if let Some(c) = cr.cpc_bid_micros {
                    m.insert("cpcBidMicros".into(), Value::String(c.to_string()));
                }
            }
            "bid_modifier" => {
                if let Some(bm) = cr.bid_modifier {
                    if let Some(n) = serde_json::Number::from_f64(bm) {
                        m.insert("bidModifier".into(), Value::Number(n));
                    }
                }
            }
            _ => {}
        }
    }
    Value::Object(m)
}

fn campaign_criterion_update_body(
    cr: &JsonCampaignCriterion,
    resource_name: &str,
    fields: &[String],
) -> Value {
    let mut m = Map::new();
    m.insert("resourceName".into(), Value::String(resource_name.to_string()));
    for f in fields {
        match f.as_str() {
            "status" => {
                if let Some(s) = &cr.status {
                    m.insert("status".into(), Value::String(s.clone()));
                }
            }
            "negative" => {
                if let Some(n) = cr.negative {
                    m.insert("negative".into(), Value::Bool(n));
                }
            }
            "bid_modifier" => {
                if let Some(bm) = cr.bid_modifier {
                    if let Some(n) = serde_json::Number::from_f64(bm) {
                        m.insert("bidModifier".into(), Value::Number(n));
                    }
                }
            }
            _ => {}
        }
    }
    Value::Object(m)
}

fn conversion_action_create(c: &JsonConversionAction, resource_name: &str) -> Value {
    let mut m = Map::new();
    m.insert("resourceName".into(), Value::String(resource_name.to_string()));
    m.insert("name".into(), Value::String(c.name.clone()));
    m.insert("type".into(), Value::String(c.ty.clone()));
    m.insert("category".into(), Value::String(c.category.clone()));
    if let Some(s) = &c.status {
        m.insert("status".into(), Value::String(s.clone()));
    }
    if let Some(ct) = &c.counting_type {
        m.insert("countingType".into(), Value::String(ct.clone()));
    }
    if let Some(d) = c.click_through_lookback_window_days {
        m.insert(
            "clickThroughLookbackWindowDays".into(),
            Value::String(d.to_string()),
        );
    }
    if let Some(d) = c.view_through_lookback_window_days {
        m.insert(
            "viewThroughLookbackWindowDays".into(),
            Value::String(d.to_string()),
        );
    }
    if let Some(vs) = &c.value_settings {
        m.insert("valueSettings".into(), value_settings_value(vs));
    }
    Value::Object(m)
}

fn value_settings_value(vs: &crate::commands::export::JsonValueSettings) -> Value {
    let mut sub = Map::new();
    if let Some(v) = vs.default_value {
        if let Some(n) = serde_json::Number::from_f64(v) {
            sub.insert("defaultValue".into(), Value::Number(n));
        }
    }
    if let Some(s) = &vs.default_currency_code {
        sub.insert("defaultCurrencyCode".into(), Value::String(s.clone()));
    }
    if let Some(b) = vs.always_use_default_value {
        sub.insert("alwaysUseDefaultValue".into(), Value::Bool(b));
    }
    Value::Object(sub)
}

fn conversion_action_update_body(
    c: &JsonConversionAction,
    resource_name: &str,
    fields: &[String],
) -> Value {
    let mut m = Map::new();
    m.insert("resourceName".into(), Value::String(resource_name.to_string()));
    let mut value_settings_sub: Option<Map<String, Value>> = None;
    for f in fields {
        match f.as_str() {
            "status" => {
                if let Some(s) = &c.status {
                    m.insert("status".into(), Value::String(s.clone()));
                }
            }
            "counting_type" => {
                if let Some(ct) = &c.counting_type {
                    m.insert("countingType".into(), Value::String(ct.clone()));
                }
            }
            "click_through_lookback_window_days" => {
                if let Some(d) = c.click_through_lookback_window_days {
                    m.insert(
                        "clickThroughLookbackWindowDays".into(),
                        Value::String(d.to_string()),
                    );
                }
            }
            "view_through_lookback_window_days" => {
                if let Some(d) = c.view_through_lookback_window_days {
                    m.insert(
                        "viewThroughLookbackWindowDays".into(),
                        Value::String(d.to_string()),
                    );
                }
            }
            "value_settings.default_value" => {
                let sub = value_settings_sub.get_or_insert_with(Map::new);
                if let Some(v) = c.value_settings.as_ref().and_then(|v| v.default_value) {
                    if let Some(n) = serde_json::Number::from_f64(v) {
                        sub.insert("defaultValue".into(), Value::Number(n));
                    }
                }
            }
            "value_settings.default_currency_code" => {
                let sub = value_settings_sub.get_or_insert_with(Map::new);
                if let Some(s) = c
                    .value_settings
                    .as_ref()
                    .and_then(|v| v.default_currency_code.clone())
                {
                    sub.insert("defaultCurrencyCode".into(), Value::String(s));
                }
            }
            "value_settings.always_use_default_value" => {
                let sub = value_settings_sub.get_or_insert_with(Map::new);
                if let Some(b) = c
                    .value_settings
                    .as_ref()
                    .and_then(|v| v.always_use_default_value)
                {
                    sub.insert("alwaysUseDefaultValue".into(), Value::Bool(b));
                }
            }
            _ => {}
        }
    }
    if let Some(sub) = value_settings_sub {
        m.insert("valueSettings".into(), Value::Object(sub));
    }
    Value::Object(m)
}

fn call_asset_create(a: &JsonCallAsset, resource_name: &str, action_rn: Option<&str>) -> Value {
    let mut m = Map::new();
    m.insert("resourceName".into(), Value::String(resource_name.to_string()));
    let mut call = Map::new();
    call.insert("countryCode".into(), Value::String(a.country_code.clone()));
    call.insert("phoneNumber".into(), Value::String(a.phone_number.clone()));
    if let Some(s) = &a.call_conversion_reporting_state {
        call.insert("callConversionReportingState".into(), Value::String(s.clone()));
    }
    if let Some(action_rn) = action_rn {
        call.insert(
            "callConversionAction".into(),
            Value::String(action_rn.to_string()),
        );
    }
    m.insert("callAsset".into(), Value::Object(call));
    Value::Object(m)
}

fn customer_asset_create(a: &JsonCustomerAsset, resource_name: &str, asset_rn: &str) -> Value {
    let mut m = Map::new();
    m.insert("resourceName".into(), Value::String(resource_name.to_string()));
    m.insert("asset".into(), Value::String(asset_rn.to_string()));
    m.insert("fieldType".into(), Value::String(a.field_type.clone()));
    if let Some(s) = &a.status {
        m.insert("status".into(), Value::String(s.clone()));
    }
    Value::Object(m)
}

fn customer_asset_update_body(
    a: &JsonCustomerAsset,
    resource_name: &str,
    fields: &[String],
) -> Value {
    let mut m = Map::new();
    m.insert("resourceName".into(), Value::String(resource_name.to_string()));
    for f in fields {
        if f == "status" {
            if let Some(s) = &a.status {
                m.insert("status".into(), Value::String(s.clone()));
            }
        }
    }
    Value::Object(m)
}

fn string_array(items: &[String]) -> Value {
    Value::Array(items.iter().cloned().map(Value::String).collect())
}

fn sitelink_asset_create(a: &JsonSitelinkAsset, resource_name: &str) -> Value {
    let mut m = Map::new();
    m.insert("resourceName".into(), Value::String(resource_name.to_string()));
    if !a.final_urls.is_empty() {
        m.insert("finalUrls".into(), string_array(&a.final_urls));
    }
    let mut s = Map::new();
    s.insert("linkText".into(), Value::String(a.link_text.clone()));
    if let Some(d) = &a.description1 {
        s.insert("description1".into(), Value::String(d.clone()));
    }
    if let Some(d) = &a.description2 {
        s.insert("description2".into(), Value::String(d.clone()));
    }
    m.insert("sitelinkAsset".into(), Value::Object(s));
    Value::Object(m)
}

fn callout_asset_create(a: &JsonCalloutAsset, resource_name: &str) -> Value {
    let mut m = Map::new();
    m.insert("resourceName".into(), Value::String(resource_name.to_string()));
    let mut c = Map::new();
    c.insert("calloutText".into(), Value::String(a.text.clone()));
    m.insert("calloutAsset".into(), Value::Object(c));
    Value::Object(m)
}

fn youtube_video_asset_create(a: &JsonYoutubeVideoAsset, resource_name: &str) -> Value {
    let mut m = Map::new();
    m.insert("resourceName".into(), Value::String(resource_name.to_string()));
    let mut y = Map::new();
    y.insert(
        "youtubeVideoId".into(),
        Value::String(a.youtube_video_id.clone()),
    );
    if let Some(t) = &a.youtube_video_title {
        y.insert("youtubeVideoTitle".into(), Value::String(t.clone()));
    }
    m.insert("youtubeVideoAsset".into(), Value::Object(y));
    Value::Object(m)
}

fn structured_snippet_asset_create(a: &JsonStructuredSnippetAsset, resource_name: &str) -> Value {
    let mut m = Map::new();
    m.insert("resourceName".into(), Value::String(resource_name.to_string()));
    let mut s = Map::new();
    s.insert("header".into(), Value::String(a.header.clone()));
    s.insert("values".into(), string_array(&a.values));
    m.insert("structuredSnippetAsset".into(), Value::Object(s));
    Value::Object(m)
}

// No resourceName: the campaignAsset id is the composite
// campaign_id~asset_id~field_type, so pinning it would leak a referenced new
// asset's temp negative id into this op's own id. The campaign / asset fields
// carry the references.
fn campaign_asset_create(a: &JsonCampaignAsset, campaign_rn: &str, asset_rn: &str) -> Value {
    let mut m = Map::new();
    m.insert("campaign".into(), Value::String(campaign_rn.to_string()));
    m.insert("asset".into(), Value::String(asset_rn.to_string()));
    m.insert("fieldType".into(), Value::String(a.field_type.clone()));
    if let Some(s) = &a.status {
        m.insert("status".into(), Value::String(s.clone()));
    }
    Value::Object(m)
}

fn campaign_asset_update_body(
    a: &JsonCampaignAsset,
    resource_name: &str,
    fields: &[String],
) -> Value {
    let mut m = Map::new();
    m.insert("resourceName".into(), Value::String(resource_name.to_string()));
    for f in fields {
        if f == "status" {
            if let Some(s) = &a.status {
                m.insert("status".into(), Value::String(s.clone()));
            }
        }
    }
    Value::Object(m)
}

fn ad_group_asset_create(a: &JsonAdGroupAsset, ad_group_rn: &str, asset_rn: &str) -> Value {
    let mut m = Map::new();
    m.insert("adGroup".into(), Value::String(ad_group_rn.to_string()));
    m.insert("asset".into(), Value::String(asset_rn.to_string()));
    m.insert("fieldType".into(), Value::String(a.field_type.clone()));
    if let Some(s) = &a.status {
        m.insert("status".into(), Value::String(s.clone()));
    }
    Value::Object(m)
}

fn ad_group_asset_update_body(
    a: &JsonAdGroupAsset,
    resource_name: &str,
    fields: &[String],
) -> Value {
    let mut m = Map::new();
    m.insert("resourceName".into(), Value::String(resource_name.to_string()));
    for f in fields {
        if f == "status" {
            if let Some(s) = &a.status {
                m.insert("status".into(), Value::String(s.clone()));
            }
        }
    }
    Value::Object(m)
}

fn shared_set_create(s: &JsonSharedSet, resource_name: &str) -> Value {
    let mut m = Map::new();
    m.insert("resourceName".into(), Value::String(resource_name.to_string()));
    m.insert("name".into(), Value::String(s.name.clone()));
    let ty = s.ty.clone().unwrap_or_else(|| "NEGATIVE_KEYWORDS".to_string());
    m.insert("type".into(), Value::String(ty));
    if let Some(st) = &s.status {
        m.insert("status".into(), Value::String(st.clone()));
    }
    Value::Object(m)
}

fn shared_set_update_body(s: &JsonSharedSet, resource_name: &str, fields: &[String]) -> Value {
    let mut m = Map::new();
    m.insert("resourceName".into(), Value::String(resource_name.to_string()));
    for f in fields {
        match f.as_str() {
            "status" => {
                if let Some(st) = &s.status {
                    m.insert("status".into(), Value::String(st.clone()));
                }
            }
            "type" => {
                if let Some(t) = &s.ty {
                    m.insert("type".into(), Value::String(t.clone()));
                }
            }
            _ => {}
        }
    }
    Value::Object(m)
}

fn shared_criterion_create(
    shared_set_rn: &str,
    kw: &crate::commands::export::JsonKeyword,
) -> Value {
    // No resourceName: the API rejects creates that pin the server-assigned criterion_id.
    let mut m = Map::new();
    m.insert("sharedSet".into(), Value::String(shared_set_rn.to_string()));
    let mut keyword = Map::new();
    keyword.insert("text".into(), Value::String(kw.text.clone()));
    keyword.insert("matchType".into(), Value::String(kw.match_type.clone()));
    m.insert("keyword".into(), Value::Object(keyword));
    Value::Object(m)
}

fn campaign_shared_set_create(
    cs: &JsonCampaignSharedSet,
    campaign_rn: &str,
    shared_set_rn: &str,
) -> Value {
    // No resourceName: the campaignSharedSet id is the composite
    // campaign_id~shared_set_id, so pinning it would leak a referenced set's
    // temp negative id into this op's own id — and the API reads any negative
    // component as a temp-id *claim*, colliding when N attachments reference
    // one new set. The campaign / sharedSet fields carry the references.
    let mut m = Map::new();
    m.insert("campaign".into(), Value::String(campaign_rn.to_string()));
    m.insert("sharedSet".into(), Value::String(shared_set_rn.to_string()));
    if let Some(st) = &cs.status {
        m.insert("status".into(), Value::String(st.clone()));
    }
    Value::Object(m)
}

fn campaign_shared_set_update_body(
    cs: &JsonCampaignSharedSet,
    resource_name: &str,
    fields: &[String],
) -> Value {
    let mut m = Map::new();
    m.insert("resourceName".into(), Value::String(resource_name.to_string()));
    for f in fields {
        if f == "status" {
            if let Some(s) = &cs.status {
                m.insert("status".into(), Value::String(s.clone()));
            }
        }
    }
    Value::Object(m)
}

fn resolve(
    refs: &HashMap<String, String>,
    address: &str,
    owner: &str,
    field: &str,
    errors: &mut Vec<PlanBuildError>,
) -> Option<String> {
    match refs.get(address) {
        Some(rn) => Some(rn.clone()),
        None => {
            errors.push(PlanBuildError {
                address: owner.to_string(),
                message: format!("unresolved reference '{address}' for field '{field}'"),
            });
            None
        }
    }
}

/// What the criterion's `audience` block resolves to on this pass.
enum AudienceRef {
    /// Nothing to resolve, or the resource name is in hand.
    Ready(Option<String>),
    /// A declared custom audience this run has not created yet — retry later.
    Pending,
    /// A reference that names nothing; the error is already recorded.
    Unresolvable,
}

fn criterion_audience_rn(
    cr: &JsonCriterion,
    refs: &HashMap<String, String>,
    create_set: &HashSet<String>,
    owner: &str,
    errors: &mut Vec<PlanBuildError>,
) -> AudienceRef {
    match cr.audience.as_ref().and_then(JsonAudience::source) {
        Some(("custom_audience", v)) if pending_custom_audience(refs, create_set, v) => {
            AudienceRef::Pending
        }
        Some(("custom_audience", v)) => {
            match resolve_ref_or_literal(refs, v, owner, "custom_audience", errors) {
                Some(rn) => AudienceRef::Ready(Some(rn)),
                None => AudienceRef::Unresolvable,
            }
        }
        Some((_, v)) => AudienceRef::Ready(Some(v.to_string())),
        None => AudienceRef::Ready(None),
    }
}

/// True when the reference names a declared custom audience whose real
/// resource name this run does not have yet — the `validateOnly` case, where
/// `CustomAudienceService` returns errors but no results.
fn pending_custom_audience(
    refs: &HashMap<String, String>,
    create_set: &HashSet<String>,
    address: &str,
) -> bool {
    !address.starts_with("customers/")
        && create_set.contains(address)
        && !refs.contains_key(address)
}

/// Resolve a reference that may be either a typed address (looked up in `refs`)
/// or a literal Google Ads resource-name string (`customers/…`), used passthrough.
fn resolve_ref_or_literal(
    refs: &HashMap<String, String>,
    address: &str,
    owner: &str,
    field: &str,
    errors: &mut Vec<PlanBuildError>,
) -> Option<String> {
    if address.starts_with("customers/") {
        return Some(address.to_string());
    }
    resolve(refs, address, owner, field, errors)
}

/// The trailing id segment of a reference — the literal's own last segment when
/// it's a `customers/…` resource name, otherwise the last segment of its
/// planned resource name (a live id or a temp negative id). `0` if unresolved.
fn id_from_ref(refs: &HashMap<String, String>, address: &str) -> String {
    let rn = if address.starts_with("customers/") {
        address
    } else {
        match refs.get(address) {
            Some(rn) => rn.as_str(),
            None => return "0".to_string(),
        }
    };
    rn.rsplit('/').next().unwrap_or("0").to_string()
}

fn plan_rn<'a>(
    refs: &'a HashMap<String, String>,
    id: &str,
    kind: &'static str,
    errors: &mut Vec<PlanBuildError>,
) -> Option<&'a String> {
    match refs.get(id) {
        Some(rn) => Some(rn),
        None => {
            errors.push(PlanBuildError {
                address: id.to_string(),
                message: format!("internal error: no planned resource name for {kind} '{id}'"),
            });
            None
        }
    }
}

fn budget_create(b: &JsonBudget, resource_name: &str) -> Value {
    let mut m = Map::new();
    m.insert("resourceName".into(), Value::String(resource_name.to_string()));
    m.insert("name".into(), Value::String(b.name.clone()));
    m.insert(
        "amountMicros".into(),
        Value::String(b.amount_micros.to_string()),
    );
    if let Some(dm) = &b.delivery_method {
        m.insert("deliveryMethod".into(), Value::String(dm.clone()));
    }
    if let Some(es) = b.explicitly_shared {
        m.insert("explicitlyShared".into(), Value::Bool(es));
    }
    Value::Object(m)
}

fn campaign_create(c: &JsonCampaign, resource_name: &str, budget_rn: &str) -> Value {
    let mut m = Map::new();
    m.insert("resourceName".into(), Value::String(resource_name.to_string()));
    m.insert("name".into(), Value::String(c.name.clone()));
    if let Some(s) = &c.status {
        m.insert("status".into(), Value::String(s.clone()));
    }
    m.insert(
        "advertisingChannelType".into(),
        Value::String(c.advertising_channel_type.clone()),
    );
    m.insert("campaignBudget".into(), Value::String(budget_rn.to_string()));
    let eu_political = c
        .contains_eu_political_advertising
        .clone()
        .unwrap_or_else(|| "DOES_NOT_CONTAIN_EU_POLITICAL_ADVERTISING".to_string());
    m.insert(
        "containsEuPoliticalAdvertising".into(),
        Value::String(eu_political),
    );
    if let Some(d) = &c.start_date {
        m.insert("startDate".into(), Value::String(d.clone()));
    }
    if let Some(d) = &c.end_date {
        m.insert("endDate".into(), Value::String(d.clone()));
    }
    if let Some((field, body)) = bidding_strategy_value(c) {
        m.insert(field.into(), body);
    }
    if let Some(ns) = &c.network_settings {
        let mut sub = Map::new();
        if let Some(v) = ns.target_google_search {
            sub.insert("targetGoogleSearch".into(), Value::Bool(v));
        }
        if let Some(v) = ns.target_search_network {
            sub.insert("targetSearchNetwork".into(), Value::Bool(v));
        }
        if let Some(v) = ns.target_content_network {
            sub.insert("targetContentNetwork".into(), Value::Bool(v));
        }
        if let Some(v) = ns.target_partner_search_network {
            sub.insert("targetPartnerSearchNetwork".into(), Value::Bool(v));
        }
        m.insert("networkSettings".into(), Value::Object(sub));
    }
    if let Some(g) = &c.geo_target_type_setting {
        let mut sub = Map::new();
        for (field, json) in crate::schema::GEO_TARGET_TYPE_FIELDS {
            if let Some(v) = g.get(field) {
                sub.insert((*json).into(), Value::String(v.to_string()));
            }
        }
        if !sub.is_empty() {
            m.insert("geoTargetTypeSetting".into(), Value::Object(sub));
        }
    }
    if !c.frequency_caps.is_empty() {
        m.insert("frequencyCaps".into(), frequency_caps_value(c));
    }
    Value::Object(m)
}

/// The campaign's chosen `campaign_bidding_strategy` member as a
/// (camelCase field, message body) pair. Setting one member of the `oneof`
/// clears the rest, so this is also the whole body of a strategy switch — the
/// mask it rides with comes from [`campaign_update_mask`].
fn bidding_strategy_value(c: &JsonCampaign) -> Option<(&'static str, Value)> {
    let body = match c.bidding_strategy()? {
        "manual_cpc" => {
            let mut sub = Map::new();
            if let Some(e) = c.manual_cpc.as_ref().and_then(|m| m.enhanced_cpc_enabled) {
                sub.insert("enhancedCpcEnabled".into(), Value::Bool(e));
            }
            return Some(("manualCpc", Value::Object(sub)));
        }
        "manual_cpm" => ("manualCpm", json!({})),
        "manual_cpv" => ("manualCpv", json!({})),
        "target_cpm" => ("targetCpm", json!({})),
        "target_cpv" => ("targetCpv", json!({})),
        _ => return None,
    };
    Some(body)
}

fn frequency_caps_value(c: &JsonCampaign) -> Value {
    Value::Array(
        c.frequency_caps
            .iter()
            .map(|f| {
                json!({
                    "key": {
                        "level": f.level_or_default(),
                        "eventType": f.event_type,
                        "timeUnit": f.time_unit,
                        "timeLength": f.time_length,
                    },
                    "cap": f.cap,
                })
            })
            .collect(),
    )
}

fn ad_group_create(g: &JsonAdGroup, resource_name: &str, campaign_rn: &str) -> Value {
    let mut m = Map::new();
    m.insert("resourceName".into(), Value::String(resource_name.to_string()));
    m.insert("name".into(), Value::String(g.name.clone()));
    m.insert("campaign".into(), Value::String(campaign_rn.to_string()));
    if let Some(s) = &g.status {
        m.insert("status".into(), Value::String(s.clone()));
    }
    if let Some(t) = &g.ty {
        m.insert("type".into(), Value::String(t.clone()));
    }
    for (field, json) in crate::schema::AD_GROUP_BID_FIELDS {
        if let Some(c) = g.bid(field) {
            m.insert((*json).into(), Value::String(c.to_string()));
        }
    }
    Value::Object(m)
}

fn ad_group_ad_create(
    a: &JsonAdGroupAd,
    resource_name: &str,
    ag_rn: &str,
    video_rns: &[String],
) -> Value {
    let mut m = Map::new();
    m.insert("resourceName".into(), Value::String(resource_name.to_string()));
    m.insert("adGroup".into(), Value::String(ag_rn.to_string()));
    if let Some(s) = &a.status {
        m.insert("status".into(), Value::String(s.clone()));
    }
    m.insert("ad".into(), ad_value(&a.ad, video_rns));
    Value::Object(m)
}

/// Resource names of the youtube video assets an ad's creative references, in
/// declaration order. `None` means the ad cannot be built — the reasons are
/// already recorded in `errors`.
fn resolve_ad_videos(
    refs: &HashMap<String, String>,
    a: &JsonAdGroupAd,
    errors: &mut Vec<PlanBuildError>,
) -> Option<Vec<String>> {
    if let Some(v) = &a.ad.video_responsive_ad {
        return resolve(refs, &v.video, &a.id, "video", errors).map(|rn| vec![rn]);
    }
    let Some(dg) = &a.ad.demand_gen_video_responsive_ad else {
        return Some(Vec::new());
    };
    let mut ok = true;
    if dg.business_name.is_none() {
        errors.push(PlanBuildError {
            address: a.id.clone(),
            message: "demand_gen_video_responsive_ad needs business_name to be created — \
                      the Google Ads API requires the advertiser/brand name on this ad type"
                .to_string(),
        });
        ok = false;
    }
    if !dg.call_to_actions.is_empty() {
        errors.push(PlanBuildError {
            address: a.id.clone(),
            message: "demand_gen_video_responsive_ad call_to_actions cannot be created: the API \
                      takes CALL_TO_ACTION asset references here, not text, and bidsmith does not \
                      model that asset type yet — drop the attribute to create the ad without a \
                      call-to-action button"
                .to_string(),
        });
        ok = false;
    }
    let mut rns = Vec::with_capacity(dg.videos.len());
    for v in &dg.videos {
        match resolve(refs, v, &a.id, "videos", errors) {
            Some(rn) => rns.push(rn),
            None => ok = false,
        }
    }
    ok.then_some(rns)
}

fn ad_value(ad: &JsonAd, video_rns: &[String]) -> Value {
    let mut m = Map::new();
    if let Some(n) = &ad.name {
        m.insert("name".into(), Value::String(n.clone()));
    }
    let urls: Vec<Value> = ad.final_urls.iter().cloned().map(Value::String).collect();
    m.insert("finalUrls".into(), Value::Array(urls));
    if let Some(rsa) = &ad.responsive_search_ad {
        m.insert("responsiveSearchAd".into(), rsa_value(rsa));
    }
    if let Some(v) = &ad.video_responsive_ad {
        m.insert("videoResponsiveAd".into(), video_ad_value(v, video_rns));
    }
    if let Some(dg) = &ad.demand_gen_video_responsive_ad {
        m.insert(
            "demandGenVideoResponsiveAd".into(),
            demand_gen_video_ad_value(dg, video_rns),
        );
    }
    Value::Object(m)
}

fn video_ad_value(v: &JsonVideoResponsiveAd, video_rns: &[String]) -> Value {
    let mut m = Map::new();
    insert_ad_text_list(&mut m, "headlines", &v.headlines);
    insert_ad_text_list(&mut m, "longHeadlines", &v.long_headlines);
    insert_ad_text_list(&mut m, "descriptions", &v.descriptions);
    insert_ad_text_list(&mut m, "callToActions", &v.call_to_actions);
    m.insert("videos".into(), ad_video_assets(video_rns));
    Value::Object(m)
}

fn demand_gen_video_ad_value(dg: &JsonDemandGenVideoResponsiveAd, video_rns: &[String]) -> Value {
    let mut m = Map::new();
    insert_ad_text_list(&mut m, "headlines", &dg.headlines);
    insert_ad_text_list(&mut m, "longHeadlines", &dg.long_headlines);
    insert_ad_text_list(&mut m, "descriptions", &dg.descriptions);
    if !video_rns.is_empty() {
        m.insert("videos".into(), ad_video_assets(video_rns));
    }
    if let Some(b) = &dg.breadcrumb1 {
        m.insert("breadcrumb1".into(), Value::String(b.clone()));
    }
    if let Some(b) = &dg.breadcrumb2 {
        m.insert("breadcrumb2".into(), Value::String(b.clone()));
    }
    if let Some(n) = &dg.business_name {
        m.insert("businessName".into(), json!({ "text": n }));
    }
    Value::Object(m)
}

fn insert_ad_text_list(m: &mut Map<String, Value>, key: &str, texts: &[String]) {
    if texts.is_empty() {
        return;
    }
    let items: Vec<Value> = texts.iter().map(|t| json!({ "text": t })).collect();
    m.insert(key.to_string(), Value::Array(items));
}

fn ad_video_assets(video_rns: &[String]) -> Value {
    Value::Array(
        video_rns
            .iter()
            .map(|rn| json!({ "asset": rn }))
            .collect(),
    )
}

fn rsa_value(rsa: &JsonResponsiveSearchAd) -> Value {
    let mut m = Map::new();
    let headlines: Vec<Value> = rsa.headlines.iter().map(asset_value).collect();
    let descriptions: Vec<Value> = rsa.descriptions.iter().map(asset_value).collect();
    m.insert("headlines".into(), Value::Array(headlines));
    m.insert("descriptions".into(), Value::Array(descriptions));
    if let Some(p) = &rsa.path1 {
        m.insert("path1".into(), Value::String(p.clone()));
    }
    if let Some(p) = &rsa.path2 {
        m.insert("path2".into(), Value::String(p.clone()));
    }
    Value::Object(m)
}

fn asset_value(asset: &JsonRsaAsset) -> Value {
    let mut m = Map::new();
    m.insert("text".into(), Value::String(asset.text.clone()));
    if let Some(p) = &asset.pin {
        m.insert("pinnedField".into(), Value::String(p.clone()));
    }
    Value::Object(m)
}

fn ad_group_criterion_create(
    cr: &JsonAdGroupCriterion,
    resource_name: &str,
    ag_rn: &str,
    audience_rn: Option<String>,
) -> Value {
    let mut m = Map::new();
    m.insert("resourceName".into(), Value::String(resource_name.to_string()));
    m.insert("adGroup".into(), Value::String(ag_rn.to_string()));
    if let Some(s) = &cr.status {
        m.insert("status".into(), Value::String(s.clone()));
    }
    if let Some(n) = cr.negative {
        m.insert("negative".into(), Value::Bool(n));
    }
    if let Some(c) = cr.cpc_bid_micros {
        m.insert("cpcBidMicros".into(), Value::String(c.to_string()));
    }
    if let Some(bm) = cr.bid_modifier {
        if let Some(n) = serde_json::Number::from_f64(bm) {
            m.insert("bidModifier".into(), Value::Number(n));
        }
    }
    insert_criterion(&mut m, &cr.target, audience_rn);
    Value::Object(m)
}

fn campaign_criterion_create(
    cr: &JsonCampaignCriterion,
    camp_rn: &str,
    audience_rn: Option<String>,
) -> Value {
    // No resourceName: a pinned composite id re-claims the new campaign's temp id, which the API rejects as a duplicate.
    let mut m = Map::new();
    m.insert("campaign".into(), Value::String(camp_rn.to_string()));
    if let Some(s) = &cr.status {
        m.insert("status".into(), Value::String(s.clone()));
    }
    if let Some(n) = cr.negative {
        m.insert("negative".into(), Value::Bool(n));
    }
    if let Some(bm) = cr.bid_modifier {
        if let Some(n) = serde_json::Number::from_f64(bm) {
            m.insert("bidModifier".into(), Value::Number(n));
        }
    }
    insert_criterion(&mut m, &cr.target, audience_rn);
    Value::Object(m)
}

/// The criterion `oneof` on a create op — the same wire shape on both criterion
/// services. `audience_rn` is the already-resolved resource name of whichever
/// audience message the `audience` block named.
fn insert_criterion(m: &mut Map<String, Value>, cr: &JsonCriterion, audience_rn: Option<String>) {
    if let Some(kw) = &cr.keyword {
        let mut sub = Map::new();
        sub.insert("text".into(), Value::String(kw.text.clone()));
        sub.insert("matchType".into(), Value::String(kw.match_type.clone()));
        m.insert("keyword".into(), Value::Object(sub));
    }
    if let Some(loc) = &cr.location {
        let mut sub = Map::new();
        sub.insert(
            "geoTargetConstant".into(),
            Value::String(loc.geo_target_constant.clone()),
        );
        m.insert("location".into(), Value::Object(sub));
    }
    if let Some(lang) = &cr.language {
        let mut sub = Map::new();
        sub.insert(
            "languageConstant".into(),
            Value::String(lang.language_constant.clone()),
        );
        m.insert("language".into(), Value::Object(sub));
    }
    if let Some(prox) = &cr.proximity {
        let mut sub = Map::new();
        let mut geo = Map::new();
        geo.insert(
            "latitudeInMicroDegrees".into(),
            Value::Number(to_micro_degrees(prox.latitude).into()),
        );
        geo.insert(
            "longitudeInMicroDegrees".into(),
            Value::Number(to_micro_degrees(prox.longitude).into()),
        );
        sub.insert("geoPoint".into(), Value::Object(geo));
        sub.insert(
            "radius".into(),
            serde_json::Number::from_f64(prox.radius)
                .map(Value::Number)
                .unwrap_or(Value::Null),
        );
        sub.insert(
            "radiusUnits".into(),
            Value::String(prox.radius_units.clone()),
        );
        m.insert("proximity".into(), Value::Object(sub));
    }
    if let Some(dev) = &cr.device {
        let mut sub = Map::new();
        sub.insert("type".into(), Value::String(dev.ty.clone()));
        m.insert("device".into(), Value::Object(sub));
    }
    if let Some(c) = &cr.youtube_channel {
        m.insert("youtubeChannel".into(), json!({ "channelId": c.channel_id }));
    }
    if let Some(v) = &cr.youtube_video {
        m.insert("youtubeVideo".into(), json!({ "videoId": v.video_id }));
    }
    if let Some(t) = &cr.topic {
        m.insert("topic".into(), json!({ "topicConstant": t.topic_constant }));
    }
    if let Some(p) = &cr.placement {
        m.insert("placement".into(), json!({ "url": p.url }));
    }
    if let Some(u) = &cr.user_interest {
        m.insert(
            "userInterest".into(),
            json!({ "userInterestCategory": u.user_interest_category }),
        );
    }
    if let Some(a) = &cr.age_range {
        m.insert("ageRange".into(), json!({ "type": a.ty }));
    }
    if let Some(g) = &cr.gender {
        m.insert("gender".into(), json!({ "type": g.ty }));
    }
    if let Some(p) = &cr.parental_status {
        m.insert("parentalStatus".into(), json!({ "type": p.ty }));
    }
    if let Some(i) = &cr.income_range {
        m.insert("incomeRange".into(), json!({ "type": i.ty }));
    }
    if let Some(audience) = &cr.audience {
        if let Some(rn) = &audience_rn {
            if audience.custom_audience.is_some() {
                m.insert("customAudience".into(), json!({ "customAudience": rn }));
            } else if audience.user_list.is_some() {
                m.insert("userList".into(), json!({ "userList": rn }));
            } else {
                m.insert(
                    "combinedAudience".into(),
                    json!({ "combinedAudience": rn }),
                );
            }
        }
    }
}

// "No resource name is expected for the new custom audience" — this service
// has no temp-id mechanism to claim one with.
fn custom_audience_create(a: &JsonCustomAudience) -> Value {
    let mut m = Map::new();
    m.insert("name".into(), Value::String(a.name.clone()));
    if let Some(d) = &a.description {
        m.insert("description".into(), Value::String(d.clone()));
    }
    if let Some(t) = &a.ty {
        m.insert("type".into(), Value::String(t.clone()));
    }
    if let Some(s) = &a.status {
        m.insert("status".into(), Value::String(s.clone()));
    }
    m.insert("members".into(), custom_audience_members_value(a));
    Value::Object(m)
}

fn custom_audience_update_body(
    a: &JsonCustomAudience,
    resource_name: &str,
    fields: &[String],
) -> Value {
    let mut m = Map::new();
    m.insert("resourceName".into(), Value::String(resource_name.to_string()));
    for f in fields {
        match f.as_str() {
            "description" => {
                if let Some(d) = &a.description {
                    m.insert("description".into(), Value::String(d.clone()));
                }
            }
            "status" => {
                if let Some(s) = &a.status {
                    m.insert("status".into(), Value::String(s.clone()));
                }
            }
            // A repeated field is replaced wholesale; an empty list clears it.
            "members" => {
                m.insert("members".into(), custom_audience_members_value(a));
            }
            _ => {}
        }
    }
    Value::Object(m)
}

fn custom_audience_members_value(a: &JsonCustomAudience) -> Value {
    Value::Array(
        a.members
            .iter()
            .filter_map(|m| {
                let (field, member_type, value) = m.payload()?;
                let key = match field {
                    "place_category" => "placeCategory",
                    other => other,
                };
                Some(json!({ "memberType": member_type, key: value }))
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::diff::{
        Action, ClaimPlanEntry, DiffReport, FieldChange, LabelPlanEntry, ResourceDiff,
    };

    /// The unified batch as it looks when no custom audience had to be created
    /// first — the shape every case but the custom-audience tests exercises.
    fn build_mutate_with_diff(
        input: &ExportInput,
        report: &DiffReport,
        validate_only: bool,
    ) -> Result<PlanBody, Vec<PlanBuildError>> {
        super::build_mutate_with_diff(input, report, validate_only, &HashMap::new())
    }

    fn create_diff(address: &str, kind: &'static str) -> ResourceDiff {
        ResourceDiff {
            address: address.to_string(),
            kind,
            action: Action::Create,
        }
    }

    fn expect_plan(result: Result<PlanBody, Vec<PlanBuildError>>) -> PlanBody {
        match result {
            Ok(plan) => plan,
            Err(errs) => panic!(
                "plan should build: {}",
                errs.iter()
                    .map(|e| format!("{}: {}", e.address, e.message))
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        }
    }

    fn campaign_with_caps(caps: Value) -> ExportInput {
        serde_json::from_value(json!({
            "customer_id": "100",
            "campaign_budgets": [{"id": "m.b", "name": "B", "amount_micros": 10000000}],
            "campaigns": [{
                "id": "m.c", "name": "V", "advertising_channel_type": "VIDEO",
                "campaign_budget": "m.b", "frequency_caps": caps
            }]
        }))
        .expect("valid ExportInput")
    }

    fn campaign_bidding(channel: &str, bidding: Value) -> ExportInput {
        let mut campaign = json!({
            "id": "m.c", "name": "V", "advertising_channel_type": channel,
            "campaign_budget": "m.b"
        });
        for (k, v) in bidding.as_object().unwrap() {
            campaign[k] = v.clone();
        }
        serde_json::from_value(json!({
            "customer_id": "100",
            "campaign_budgets": [{"id": "m.b", "name": "B", "amount_micros": 10000000}],
            "campaigns": [campaign]
        }))
        .expect("valid ExportInput")
    }

    fn video_campaign_bidding(bidding: Value) -> ExportInput {
        campaign_bidding("VIDEO", bidding)
    }

    /// A campaign whose only drift is the bidding block it declares.
    fn strategy_switch(changed_field: &str) -> DiffReport {
        DiffReport {
            diffs: vec![
                ResourceDiff {
                    address: "m.b".to_string(),
                    kind: "campaign_budget",
                    action: Action::NoOp { live_id: "41".to_string() },
                },
                ResourceDiff {
                    address: "m.c".to_string(),
                    kind: "campaign",
                    action: Action::Update {
                        live_id: "42".to_string(),
                        changed_fields: vec![FieldChange::named(changed_field)],
                    },
                },
            ],
            label_plans: Vec::new(),
            claim_plans: Vec::new(),
            noop_count: 1,
            create_count: 0,
            update_count: 1,
            delete_count: 0,
            adopt_count: 0,
            ..DiffReport::default()
        }
    }

    fn campaign_update_op(input: &ExportInput, report: &DiffReport) -> Value {
        let plan = expect_plan(build_mutate_with_diff(input, report, true));
        plan.body["mutateOperations"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|op| op.get("campaignOperation").cloned())
            .expect("campaign update op")
    }

    #[test]
    fn video_bidding_strategies_create_as_empty_messages() {
        for (field, api_field) in [
            ("manual_cpm", "manualCpm"),
            ("manual_cpv", "manualCpv"),
            ("target_cpm", "targetCpm"),
            ("target_cpv", "targetCpv"),
        ] {
            let input = video_campaign_bidding(json!({ field: {} }));
            let report = DiffReport {
                diffs: vec![
                    create_diff("m.b", "campaign_budget"),
                    create_diff("m.c", "campaign"),
                ],
                label_plans: Vec::new(),
                claim_plans: Vec::new(),
                noop_count: 0,
                create_count: 2,
                update_count: 0,
                delete_count: 0,
                adopt_count: 0,
                ..DiffReport::default()
            };
            let plan = expect_plan(build_mutate_with_diff(&input, &report, true));
            let ops = plan.body["mutateOperations"].as_array().unwrap();
            let campaign = ops
                .iter()
                .find_map(|op| op.get("campaignOperation").and_then(|o| o.get("create")))
                .expect("campaign create op");
            assert_eq!(campaign[api_field], json!({}), "create body: {campaign}");
            assert!(campaign.get("manualCpc").is_none(), "{field}: {campaign}");
        }
    }

    /// A strategy the API models as a field-less message is the one case where
    /// the mask can name the `oneof` member itself.
    #[test]
    fn switching_to_a_field_less_strategy_masks_the_oneof_member() {
        let op = campaign_update_op(
            &video_campaign_bidding(json!({ "target_cpv": {} })),
            &strategy_switch("target_cpv"),
        );
        assert_eq!(op["updateMask"], json!("target_cpv"));
        assert_eq!(op["update"]["targetCpv"], json!({}));
    }

    /// Google Ads rejects a mask naming a message field that has subfields, so
    /// a switch onto one has to name the subfields instead (issue #120).
    #[test]
    fn switching_to_a_strategy_with_subfields_masks_those_subfields() {
        for (bidding, mask) in [
            (json!({ "manual_cpc": {} }), "manual_cpc.enhanced_cpc_enabled"),
            (
                json!({ "manual_cpc": {"enhanced_cpc_enabled": false} }),
                "manual_cpc.enhanced_cpc_enabled",
            ),
            (json!({ "target_cpm": {} }), "target_cpm.target_frequency_goal"),
        ] {
            let field = bidding.as_object().unwrap().keys().next().unwrap().clone();
            let op = campaign_update_op(
                &campaign_bidding("SEARCH", bidding),
                &strategy_switch(&field),
            );
            assert_eq!(op["updateMask"], json!(mask), "switch to {field}: {op}");
        }
    }

    #[test]
    fn a_declared_geo_target_type_goes_out_as_its_own_leaf_path() {
        let op = campaign_update_op(
            &campaign_bidding(
                "SEARCH",
                json!({ "geo_target_type_setting": {"positive_geo_target_type": "PRESENCE"} }),
            ),
            &strategy_switch("geo_target_type_setting.positive_geo_target_type"),
        );
        assert_eq!(
            op["updateMask"],
            json!("geo_target_type_setting.positive_geo_target_type")
        );
        assert_eq!(
            op["update"]["geoTargetTypeSetting"],
            json!({"positiveGeoTargetType": "PRESENCE"})
        );
    }

    #[test]
    fn a_new_campaign_creates_with_the_geo_target_types_it_declares() {
        let input = campaign_bidding(
            "SEARCH",
            json!({
                "manual_cpc": {},
                "geo_target_type_setting": {
                    "positive_geo_target_type": "PRESENCE",
                    "negative_geo_target_type": "PRESENCE",
                }
            }),
        );
        let report = DiffReport {
            diffs: vec![
                create_diff("m.b", "campaign_budget"),
                create_diff("m.c", "campaign"),
            ],
            create_count: 2,
            ..DiffReport::default()
        };
        let plan = expect_plan(build_mutate_with_diff(&input, &report, true));
        let campaign = plan.body["mutateOperations"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|op| op.get("campaignOperation").and_then(|o| o.get("create")))
            .expect("campaign create op");
        assert_eq!(
            campaign["geoTargetTypeSetting"],
            json!({"positiveGeoTargetType": "PRESENCE", "negativeGeoTargetType": "PRESENCE"})
        );
    }

    /// The body still carries the whole member — setting it is what clears the
    /// strategy the campaign was live on.
    #[test]
    fn switching_to_manual_cpc_still_sends_the_member_body() {
        let op = campaign_update_op(
            &campaign_bidding("SEARCH", json!({ "manual_cpc": {"enhanced_cpc_enabled": false} })),
            &strategy_switch("manual_cpc"),
        );
        assert_eq!(op["update"]["manualCpc"], json!({"enhancedCpcEnabled": false}));

        let op = campaign_update_op(
            &campaign_bidding("SEARCH", json!({ "manual_cpc": {} })),
            &strategy_switch("manual_cpc"),
        );
        assert_eq!(op["update"]["manualCpc"], json!({}));
    }

    #[test]
    fn a_strategy_switch_expands_alongside_the_other_changed_fields() {
        let input = campaign_bidding("SEARCH", json!({ "manual_cpc": {} }));
        let mut report = strategy_switch("manual_cpc");
        report.diffs[1].action = Action::Update {
            live_id: "42".to_string(),
            changed_fields: vec![FieldChange::named("name"), FieldChange::named("manual_cpc")],
        };
        let op = campaign_update_op(&input, &report);
        assert_eq!(
            op["updateMask"],
            json!("name,manual_cpc.enhanced_cpc_enabled")
        );
    }

    /// The bid amount lives on the ad group, and for a CPV strategy it is
    /// `target_cpv_micros` — not `cpc_bid_micros`, which stays zero (issue #109).
    #[test]
    fn ad_group_target_cpv_bid_update_sends_the_matching_json_field() {
        let input: ExportInput = serde_json::from_value(json!({
            "customer_id": "100",
            "campaign_budgets": [{"id": "m.b", "name": "B", "amount_micros": 10000000}],
            "campaigns": [{
                "id": "m.c", "name": "V", "advertising_channel_type": "DEMAND_GEN",
                "campaign_budget": "m.b"
            }],
            "ad_groups": [{
                "id": "m.g", "name": "AG", "campaign": "m.c",
                "cpc_bid_micros": 0, "target_cpv_micros": 60000
            }]
        }))
        .expect("valid ExportInput");
        let report = DiffReport {
            diffs: vec![
                ResourceDiff {
                    address: "m.b".to_string(),
                    kind: "campaign_budget",
                    action: Action::NoOp { live_id: "41".to_string() },
                },
                ResourceDiff {
                    address: "m.c".to_string(),
                    kind: "campaign",
                    action: Action::NoOp { live_id: "42".to_string() },
                },
                ResourceDiff {
                    address: "m.g".to_string(),
                    kind: "ad_group",
                    action: Action::Update {
                        live_id: "43".to_string(),
                        changed_fields: vec![FieldChange::named("target_cpv_micros")],
                    },
                },
            ],
            label_plans: Vec::new(),
            claim_plans: Vec::new(),
            noop_count: 2,
            create_count: 0,
            update_count: 1,
            delete_count: 0,
            adopt_count: 0,
            ..DiffReport::default()
        };
        let plan = expect_plan(build_mutate_with_diff(&input, &report, true));
        let ops = plan.body["mutateOperations"].as_array().unwrap();
        let op = ops
            .iter()
            .find_map(|op| op.get("adGroupOperation"))
            .expect("ad group update op");
        assert_eq!(op["updateMask"], json!("target_cpv_micros"));
        assert_eq!(op["update"]["targetCpvMicros"], json!("60000"));
        assert!(
            op["update"].get("cpcBidMicros").is_none(),
            "an unchanged bid field must stay out of the update body: {op}"
        );
    }

    #[test]
    fn frequency_caps_create_nests_the_key_and_defaults_the_level() {
        let input = campaign_with_caps(json!([
            {"event_type": "IMPRESSION", "time_unit": "DAY", "time_length": 1, "cap": 3},
            {"event_type": "VIDEO_VIEW", "time_unit": "WEEK", "time_length": 2, "cap": 1,
             "level": "AD_GROUP"}
        ]));
        let report = DiffReport {
            diffs: vec![
                create_diff("m.b", "campaign_budget"),
                create_diff("m.c", "campaign"),
            ],
            label_plans: Vec::new(),
            claim_plans: Vec::new(),
            noop_count: 0,
            create_count: 2,
            update_count: 0,
            delete_count: 0,
            adopt_count: 0,
            ..DiffReport::default()
        };
        let plan = expect_plan(build_mutate_with_diff(&input, &report, true));
        let ops = plan.body["mutateOperations"].as_array().unwrap();
        let campaign = ops
            .iter()
            .find_map(|op| op.get("campaignOperation").and_then(|o| o.get("create")))
            .expect("campaign create op");
        assert_eq!(
            campaign["frequencyCaps"],
            json!([
                {"key": {"level": "CAMPAIGN", "eventType": "IMPRESSION",
                         "timeUnit": "DAY", "timeLength": 1}, "cap": 3},
                {"key": {"level": "AD_GROUP", "eventType": "VIDEO_VIEW",
                         "timeUnit": "WEEK", "timeLength": 2}, "cap": 1}
            ]),
            "create body: {campaign}"
        );
    }

    #[test]
    fn clearing_every_cap_sends_an_empty_list_under_the_whole_field_mask() {
        let input = campaign_with_caps(json!([]));
        let report = DiffReport {
            diffs: vec![
                ResourceDiff {
                    address: "m.b".to_string(),
                    kind: "campaign_budget",
                    action: Action::NoOp { live_id: "41".to_string() },
                },
                ResourceDiff {
                    address: "m.c".to_string(),
                    kind: "campaign",
                    action: Action::Update {
                        live_id: "42".to_string(),
                        changed_fields: vec![FieldChange::named("frequency_caps")],
                    },
                },
            ],
            label_plans: Vec::new(),
            claim_plans: Vec::new(),
            noop_count: 1,
            create_count: 0,
            update_count: 1,
            delete_count: 0,
            adopt_count: 0,
            ..DiffReport::default()
        };
        let plan = expect_plan(build_mutate_with_diff(&input, &report, true));
        let op = plan.body["mutateOperations"][0]["campaignOperation"].clone();
        assert_eq!(op["updateMask"], json!("frequency_caps"));
        assert_eq!(op["update"]["frequencyCaps"], json!([]));
    }

    fn segment_and_criterion() -> (ExportInput, DiffReport) {
        let input: ExportInput = serde_json::from_value(json!({
            "customer_id": "100",
            "custom_audiences": [{
                "id": "m.seg", "name": "Ad blocker searchers", "type": "SEARCH",
                "description": "Search-intent segment",
                "members": [{"keyword": "ad blocker"}, {"url": "https://example.com/privacy"}]
            }],
            "campaign_criteria": [{
                "id": "m.cr", "campaign": "m.c",
                "audience": {"custom_audience": "m.seg"}
            }]
        }))
        .expect("valid ExportInput");

        let report = DiffReport {
            diffs: vec![
                ResourceDiff {
                    address: "m.c".to_string(),
                    kind: "campaign",
                    action: Action::NoOp { live_id: "42".to_string() },
                },
                create_diff("m.seg", "custom_audience"),
                create_diff("m.cr", "campaign_criterion"),
            ],
            label_plans: Vec::new(),
            claim_plans: Vec::new(),
            noop_count: 1,
            create_count: 2,
            update_count: 0,
            delete_count: 0,
            adopt_count: 0,
            ..DiffReport::default()
        };
        (input, report)
    }

    #[test]
    fn a_new_segment_goes_to_its_own_service_not_the_unified_batch() {
        // Issue #105: `MutateOperation` has no `custom_audience_operation`, so
        // one in the batch takes the whole atomic request down at parse time.
        let (input, report) = segment_and_criterion();
        let pass = build_custom_audience_mutate(&input, &report, true).expect("pre-batch call");
        assert_eq!(pass.endpoint, "customAudiences:mutate");
        assert_eq!(pass.operations.len(), 1);
        assert_eq!(pass.operations[0].address, "m.seg");

        let seg = &pass.body["operations"][0]["create"];
        assert_eq!(seg["type"], json!("SEARCH"));
        assert!(
            seg.get("resourceName").is_none(),
            "CustomAudienceService has no temp ids to claim: {seg}"
        );
        assert_eq!(
            seg["members"],
            json!([
                {"memberType": "KEYWORD", "keyword": "ad blocker"},
                {"memberType": "URL", "url": "https://example.com/privacy"}
            ]),
            "segment body: {seg}"
        );

        let plan = expect_plan(build_mutate_with_diff(&input, &report, true));
        let ops = plan.body["mutateOperations"].as_array().unwrap();
        assert!(
            ops.iter().all(|op| op.get("customAudienceOperation").is_none()),
            "{ops:#?}"
        );
    }

    #[test]
    fn a_criterion_targets_the_segment_the_first_call_created() {
        let (input, report) = segment_and_criterion();
        let created = HashMap::from([(
            "m.seg".to_string(),
            "customers/100/customAudiences/777".to_string(),
        )]);
        let plan =
            expect_plan(super::build_mutate_with_diff(&input, &report, false, &created));
        let ops = plan.body["mutateOperations"].as_array().unwrap();
        let cr = ops
            .iter()
            .find_map(|op| op.get("campaignCriterionOperation").and_then(|o| o.get("create")))
            .expect("criterion op");
        assert_eq!(
            cr["customAudience"]["customAudience"],
            json!("customers/100/customAudiences/777")
        );
        assert!(plan.deferred.is_empty(), "{:?}", plan.deferred);
    }

    #[test]
    fn a_criterion_waits_when_its_segment_has_no_resource_name_yet() {
        // The validateOnly pre-flight: CustomAudienceService returns errors but
        // no results, so there is nothing for the criterion to point at.
        let (input, report) = segment_and_criterion();
        let plan = expect_plan(build_mutate_with_diff(&input, &report, true));
        assert_eq!(plan.deferred, vec!["m.cr".to_string()]);
        let ops = plan.body["mutateOperations"].as_array().unwrap();
        assert!(
            ops.iter().all(|op| op.get("campaignCriterionOperation").is_none()),
            "a criterion with an unresolvable audience must not sink the batch: {ops:#?}"
        );
    }

    #[test]
    fn placement_and_demographic_criteria_nest_under_their_api_message() {
        let input: ExportInput = serde_json::from_value(json!({
            "customer_id": "100",
            "campaign_criteria": [
                {"id": "m.ch", "campaign": "m.c", "youtube_channel": {"channel_id": "UCabc"}},
                {"id": "m.vid", "campaign": "m.c", "youtube_video": {"video_id": "dQw4w9WgXcQ"}},
                {"id": "m.top", "campaign": "m.c", "topic": {"topic_constant": "topicConstants/278"}},
                {"id": "m.int", "campaign": "m.c",
                 "user_interest": {"user_interest_category": "userInterestConstants/80546"}},
                {"id": "m.age", "campaign": "m.c", "negative": true,
                 "age_range": {"type": "AGE_RANGE_18_24"}},
                {"id": "m.list", "campaign": "m.c",
                 "audience": {"user_list": "customers/100/userLists/987"}}
            ]
        }))
        .expect("valid ExportInput");

        let mut diffs = vec![ResourceDiff {
            address: "m.c".to_string(),
            kind: "campaign",
            action: Action::NoOp { live_id: "42".to_string() },
        }];
        for id in ["m.ch", "m.vid", "m.top", "m.int", "m.age", "m.list"] {
            diffs.push(create_diff(id, "campaign_criterion"));
        }
        let report = DiffReport {
            diffs,
            label_plans: Vec::new(),
            claim_plans: Vec::new(),
            noop_count: 1,
            create_count: 6,
            update_count: 0,
            delete_count: 0,
            adopt_count: 0,
            ..DiffReport::default()
        };
        let plan = expect_plan(build_mutate_with_diff(&input, &report, true));
        let creates: Vec<&Value> = plan.body["mutateOperations"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|op| op.get("campaignCriterionOperation")?.get("create"))
            .collect();
        assert_eq!(creates.len(), 6);
        assert_eq!(creates[0]["youtubeChannel"], json!({"channelId": "UCabc"}));
        assert_eq!(creates[1]["youtubeVideo"], json!({"videoId": "dQw4w9WgXcQ"}));
        assert_eq!(creates[2]["topic"], json!({"topicConstant": "topicConstants/278"}));
        assert_eq!(
            creates[3]["userInterest"],
            json!({"userInterestCategory": "userInterestConstants/80546"})
        );
        assert_eq!(creates[4]["ageRange"], json!({"type": "AGE_RANGE_18_24"}));
        assert_eq!(creates[4]["negative"], json!(true));
        assert_eq!(
            creates[5]["userList"],
            json!({"userList": "customers/100/userLists/987"})
        );
    }

    #[test]
    fn an_ad_group_criterion_carries_every_targeting_axis_it_declares() {
        // Issue #110: cohort narrowing belongs on the ad group for video, so the
        // ad-group service has to accept the same criterion messages the
        // campaign one does.
        let input: ExportInput = serde_json::from_value(json!({
            "customer_id": "100",
            "ad_group_criteria": [
                {"id": "m.aud", "ad_group": "m.ag",
                 "audience": {"user_list": "customers/100/userLists/987"}},
                {"id": "m.age", "ad_group": "m.ag", "bid_modifier": 1.2,
                 "age_range": {"type": "AGE_RANGE_35_44"}},
                {"id": "m.place", "ad_group": "m.ag", "negative": true,
                 "placement": {"url": "https://example.com/x"}},
                {"id": "m.geo", "ad_group": "m.ag",
                 "location": {"geo_target_constant": "geoTargetConstants/2702"}},
                {"id": "m.income", "ad_group": "m.ag",
                 "income_range": {"type": "INCOME_RANGE_90_UP"}},
                {"id": "m.parent", "ad_group": "m.ag",
                 "parental_status": {"type": "NOT_A_PARENT"}}
            ]
        }))
        .expect("valid ExportInput");

        let mut diffs = vec![ResourceDiff {
            address: "m.ag".to_string(),
            kind: "ad_group",
            action: Action::NoOp { live_id: "42".to_string() },
        }];
        for id in ["m.aud", "m.age", "m.place", "m.geo", "m.income", "m.parent"] {
            diffs.push(create_diff(id, "ad_group_criterion"));
        }
        let report = DiffReport {
            diffs,
            label_plans: Vec::new(),
            claim_plans: Vec::new(),
            noop_count: 1,
            create_count: 6,
            update_count: 0,
            delete_count: 0,
            adopt_count: 0,
            ..DiffReport::default()
        };
        let plan = expect_plan(build_mutate_with_diff(&input, &report, true));
        let creates: Vec<&Value> = plan.body["mutateOperations"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|op| op.get("adGroupCriterionOperation")?.get("create"))
            .collect();
        assert_eq!(creates.len(), 6);
        assert_eq!(
            creates[0]["userList"],
            json!({"userList": "customers/100/userLists/987"})
        );
        assert_eq!(creates[1]["ageRange"], json!({"type": "AGE_RANGE_35_44"}));
        assert_eq!(creates[1]["bidModifier"], json!(1.2));
        assert_eq!(creates[2]["placement"], json!({"url": "https://example.com/x"}));
        assert_eq!(creates[2]["negative"], json!(true));
        assert_eq!(
            creates[3]["location"],
            json!({"geoTargetConstant": "geoTargetConstants/2702"})
        );
        assert_eq!(creates[4]["incomeRange"], json!({"type": "INCOME_RANGE_90_UP"}));
        assert_eq!(creates[5]["parentalStatus"], json!({"type": "NOT_A_PARENT"}));
    }

    #[test]
    fn an_ad_group_criterion_waits_for_the_segment_it_targets() {
        let input: ExportInput = serde_json::from_value(json!({
            "customer_id": "100",
            "custom_audiences": [{
                "id": "m.seg", "name": "Ad blocker searchers", "type": "SEARCH",
                "members": [{"keyword": "ad blocker"}]
            }],
            "ad_group_criteria": [{
                "id": "m.cr", "ad_group": "m.ag",
                "audience": {"custom_audience": "m.seg"}
            }]
        }))
        .expect("valid ExportInput");
        let report = DiffReport {
            diffs: vec![
                ResourceDiff {
                    address: "m.ag".to_string(),
                    kind: "ad_group",
                    action: Action::NoOp { live_id: "42".to_string() },
                },
                create_diff("m.seg", "custom_audience"),
                create_diff("m.cr", "ad_group_criterion"),
            ],
            label_plans: Vec::new(),
            claim_plans: Vec::new(),
            noop_count: 1,
            create_count: 2,
            update_count: 0,
            delete_count: 0,
            adopt_count: 0,
            ..DiffReport::default()
        };

        let plan = expect_plan(build_mutate_with_diff(&input, &report, true));
        assert_eq!(plan.deferred, vec!["m.cr".to_string()]);
        assert!(
            plan.body["mutateOperations"]
                .as_array()
                .unwrap()
                .iter()
                .all(|op| op.get("adGroupCriterionOperation").is_none()),
            "a criterion with an unresolvable audience must not sink the batch"
        );

        let created = HashMap::from([(
            "m.seg".to_string(),
            "customers/100/customAudiences/777".to_string(),
        )]);
        let plan = expect_plan(super::build_mutate_with_diff(&input, &report, false, &created));
        let cr = plan.body["mutateOperations"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|op| op.get("adGroupCriterionOperation")?.get("create"))
            .expect("criterion op");
        assert_eq!(
            cr["customAudience"]["customAudience"],
            json!("customers/100/customAudiences/777")
        );
    }

    #[test]
    fn destroys_are_ordered_before_creates() {
        // Issue #74: replacing an RSA in an ad group already at the 3-enabled cap
        // is a destroy of the old body + a create of the new one. The API applies
        // an atomic mutate's ops in array order and checks the per-ad-group
        // enabled cap against the running state, so the remove must precede the
        // create or the batch transiently hits 5 enabled and the whole thing is
        // rejected.
        let input: ExportInput = serde_json::from_value(json!({
            "customer_id": "100",
            "ad_group_ads": [{
                "id": "m.new_ad",
                "ad_group": "m.ag",
                "ad": {
                    "final_urls": ["https://example.com"],
                    "responsive_search_ad": {
                        "headlines": [{"text": "Fresh copy"}],
                        "descriptions": [{"text": "Brand new."}]
                    }
                }
            }]
        }))
        .expect("valid ExportInput");

        let report = DiffReport {
            diffs: vec![
                ResourceDiff {
                    address: "m.ag".to_string(),
                    kind: "ad_group",
                    action: Action::NoOp { live_id: "300".to_string() },
                },
                create_diff("m.new_ad", "ad_group_ad"),
                ResourceDiff {
                    address: "old ad (managed, no longer declared)".to_string(),
                    kind: "ad_group_ad",
                    action: Action::Delete { live_id: "900".to_string() },
                },
            ],
            label_plans: Vec::new(),
            claim_plans: Vec::new(),
            noop_count: 1,
            create_count: 1,
            update_count: 0,
            delete_count: 1,
            adopt_count: 0,
            ..DiffReport::default()
        };

        let plan = match build_mutate_with_diff(&input, &report, true) {
            Ok(plan) => plan,
            Err(errs) => panic!(
                "plan should build: {}",
                errs.iter()
                    .map(|e| format!("{}: {}", e.address, e.message))
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        };
        let ops = plan.body["mutateOperations"].as_array().unwrap();

        let remove_idx = ops
            .iter()
            .position(|o| o.get("adGroupAdOperation").and_then(|x| x.get("remove")).is_some())
            .expect("a remove op");
        let create_idx = ops
            .iter()
            .position(|o| o.get("adGroupAdOperation").and_then(|x| x.get("create")).is_some())
            .expect("a create op");
        assert!(
            remove_idx < create_idx,
            "destroy must precede create: remove at {remove_idx}, create at {create_idx}"
        );

        // operations[] stays index-aligned with mutateOperations[] so the plan can
        // attribute a per-op error back to its address via op_index.
        assert_eq!(plan.operations.len(), ops.len());
        assert_eq!(plan.operations[create_idx].address, "m.new_ad");
    }

    #[test]
    fn label_plan_emits_create_label_and_association() {
        let input: ExportInput = serde_json::from_value(json!({
            "customer_id": "100",
            "campaigns": [{
                "id": "m.c",
                "name": "C",
                "advertising_channel_type": "SEARCH",
                "campaign_budget": "customers/100/campaignBudgets/999"
            }]
        }))
        .expect("valid ExportInput");

        let report = DiffReport {
            diffs: vec![ResourceDiff {
                address: "m.c".to_string(),
                kind: "campaign",
                action: Action::NoOp { live_id: "555".to_string() },
            }],
            label_plans: vec![LabelPlanEntry {
                address: "m.c".to_string(),
                kind: "campaign",
                label_address: "m.c".to_string(),
                existing_label_rn: None,
                stale_assoc_rn: Some("customers/100/campaignLabels/555~111".to_string()),
            }],
            claim_plans: Vec::new(),
            noop_count: 1,
            create_count: 0,
            update_count: 0,
            delete_count: 0,
            adopt_count: 1,
            ..DiffReport::default()
        };

        let plan = match build_mutate_with_diff(&input, &report, true) {
            Ok(plan) => plan,
            Err(errs) => panic!(
                "plan should build: {}",
                errs.iter()
                    .map(|e| format!("{}: {}", e.address, e.message))
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        };
        let ops = plan.body["mutateOperations"].as_array().unwrap();

        let label = ops
            .iter()
            .find_map(|o| o.get("labelOperation").and_then(|x| x.get("create")))
            .expect("a label create op");
        assert_eq!(label["name"].as_str().unwrap(), "bidsmith:address=m.c");
        let label_rn = label["resourceName"].as_str().unwrap();
        assert!(label_rn.contains("/labels/-"), "new label gets a temp id");

        let assoc = ops
            .iter()
            .find_map(|o| o.get("campaignLabelOperation").and_then(|x| x.get("create")))
            .expect("a campaign label association create");
        assert_eq!(assoc["campaign"].as_str().unwrap(), "customers/100/campaigns/555");
        assert_eq!(assoc["label"].as_str().unwrap(), label_rn);

        let removed = ops
            .iter()
            .find_map(|o| o.get("campaignLabelOperation").and_then(|x| x.get("remove")))
            .expect("the stale association is removed");
        assert_eq!(removed.as_str().unwrap(), "customers/100/campaignLabels/555~111");
    }

    #[test]
    fn delete_diff_emits_a_remove_op() {
        let input: ExportInput = serde_json::from_value(json!({
            "customer_id": "6571974784",
        }))
        .expect("valid ExportInput");

        let report = DiffReport {
            diffs: vec![
                ResourceDiff {
                    address: "ag (removed negative_keyword \"cheap\" BROAD)".to_string(),
                    kind: "ad_group_criterion",
                    action: Action::Delete { live_id: "3~101".to_string() },
                },
                ResourceDiff {
                    address: "set (removed negative_keyword \"globex\" BROAD)".to_string(),
                    kind: "shared_criterion",
                    action: Action::Delete { live_id: "50~201".to_string() },
                },
            ],
            label_plans: Vec::new(),
            claim_plans: Vec::new(),
            noop_count: 0,
            create_count: 0,
            update_count: 0,
            delete_count: 2,
            adopt_count: 0,
            ..DiffReport::default()
        };

        let plan = match build_mutate_with_diff(&input, &report, true) {
            Ok(plan) => plan,
            Err(errs) => panic!(
                "plan should build: {}",
                errs.iter()
                    .map(|e| format!("{}: {}", e.address, e.message))
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        };
        let ops = plan.body["mutateOperations"].as_array().unwrap();
        assert_eq!(ops.len(), 2, "expected two remove ops, got {ops:?}");

        let agc = ops
            .iter()
            .find_map(|op| op.get("adGroupCriterionOperation").and_then(|o| o.get("remove")))
            .expect("ad group criterion remove op");
        assert_eq!(
            agc.as_str().unwrap(),
            "customers/6571974784/adGroupCriteria/3~101"
        );

        let sc = ops
            .iter()
            .find_map(|op| op.get("sharedCriterionOperation").and_then(|o| o.get("remove")))
            .expect("shared criterion remove op");
        assert_eq!(
            sc.as_str().unwrap(),
            "customers/6571974784/sharedCriteria/50~201"
        );
    }

    #[test]
    fn new_shared_set_members_create_without_id() {
        let input: ExportInput = serde_json::from_value(json!({
            "customer_id": "6571974784",
            "shared_sets": [{
                "id": "account.google_ads_shared_set.platform_negative_keywords",
                "name": "platform campaigns negative keywords",
                "type": "NEGATIVE_KEYWORDS",
                "status": "ENABLED"
            }],
            "shared_criteria": [{
                "id": "account.google_ads_shared_set.platform_negative_keywords~0",
                "shared_set": "account.google_ads_shared_set.platform_negative_keywords",
                "keyword": { "text": "ad blocker detector", "match_type": "PHRASE" }
            }]
        }))
        .expect("valid ExportInput");

        let report = DiffReport {
            diffs: vec![
                create_diff(
                    "account.google_ads_shared_set.platform_negative_keywords",
                    "shared_set",
                ),
                create_diff(
                    "account.google_ads_shared_set.platform_negative_keywords~0",
                    "shared_criterion",
                ),
            ],
            label_plans: Vec::new(),
            claim_plans: Vec::new(),
            noop_count: 0,
            create_count: 2,
            update_count: 0,
            delete_count: 0,
            adopt_count: 0,
            ..DiffReport::default()
        };

        let plan = match build_mutate_with_diff(&input, &report, true) {
            Ok(plan) => plan,
            Err(errs) => panic!(
                "plan should build: {}",
                errs.iter()
                    .map(|e| format!("{}: {}", e.address, e.message))
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        };
        let ops = plan.body["mutateOperations"].as_array().unwrap();

        let set_create = ops
            .iter()
            .find_map(|op| op.get("sharedSetOperation").and_then(|o| o.get("create")))
            .expect("shared set create op");
        let set_rn = set_create["resourceName"].as_str().unwrap();
        assert!(
            set_rn.contains("/sharedSets/-"),
            "new set should get a temp negative resource name, got {set_rn}"
        );

        let crit_create = ops
            .iter()
            .find_map(|op| {
                op.get("sharedCriterionOperation")
                    .and_then(|o| o.get("create"))
            })
            .expect("shared criterion create op");

        assert!(
            crit_create.get("resourceName").is_none(),
            "shared criterion create must not pin an id/resource_name"
        );
        assert_eq!(
            crit_create["sharedSet"].as_str().unwrap(),
            set_rn,
            "member must reference the parent set's temp resource name"
        );
    }

    #[test]
    fn new_campaign_criteria_do_not_reclaim_the_campaign_temp_id() {
        // Green-field US/English campaign: location/language criteria have no own negative id, so a pinned composite resource_name re-claimed the new campaign's temp id (-2) and the API rejected the whole batch.
        let input: ExportInput = serde_json::from_value(json!({
            "customer_id": "6571974784",
            "campaign_budgets": [{"id": "m.b", "name": "B", "amount_micros": 15000000}],
            "campaigns": [{
                "id": "m.c", "name": "HBO", "advertising_channel_type": "SEARCH",
                "campaign_budget": "m.b"
            }],
            "campaign_criteria": [
                {"id": "m.loc", "campaign": "m.c", "location": {"geo_target_constant": "geoTargetConstants/2840"}},
                {"id": "m.lang", "campaign": "m.c", "language": {"language_constant": "languageConstants/1000"}},
                {"id": "m.neg", "campaign": "m.c", "negative": true, "keyword": {"text": "free", "match_type": "BROAD"}}
            ]
        }))
        .expect("valid ExportInput");

        let report = DiffReport {
            diffs: vec![
                create_diff("m.b", "campaign_budget"),
                create_diff("m.c", "campaign"),
                create_diff("m.loc", "campaign_criterion"),
                create_diff("m.lang", "campaign_criterion"),
                create_diff("m.neg", "campaign_criterion"),
            ],
            label_plans: Vec::new(),
            claim_plans: Vec::new(),
            noop_count: 0,
            create_count: 5,
            update_count: 0,
            delete_count: 0,
            adopt_count: 0,
            ..DiffReport::default()
        };

        let plan = match build_mutate_with_diff(&input, &report, true) {
            Ok(plan) => plan,
            Err(errs) => panic!(
                "plan should build: {}",
                errs.iter()
                    .map(|e| format!("{}: {}", e.address, e.message))
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        };
        let ops = plan.body["mutateOperations"].as_array().unwrap();

        let campaign_rn = ops
            .iter()
            .find_map(|op| op.get("campaignOperation").and_then(|o| o.get("create")))
            .and_then(|c| c.get("resourceName"))
            .and_then(Value::as_str)
            .expect("campaign create op");
        assert_eq!(campaign_rn, "customers/6571974784/campaigns/-2");

        let criteria: Vec<&Value> = ops
            .iter()
            .filter_map(|op| {
                op.get("campaignCriterionOperation").and_then(|o| o.get("create"))
            })
            .collect();
        assert_eq!(criteria.len(), 3, "every declared criterion emits a create op");

        for create in &criteria {
            assert!(
                create.get("resourceName").is_none(),
                "criterion create must not pin a resource_name that re-claims the campaign temp id: {create}"
            );
            assert_eq!(
                create["campaign"].as_str().unwrap(),
                campaign_rn,
                "the criterion references the campaign by its temp resource name"
            );
        }

        // The campaign temp id (-2) is claimed exactly once across the batch: by
        // the campaign op itself. No other op pins it in its own id.
        let claims = ops
            .iter()
            .filter_map(|op| op.as_object())
            .flat_map(|m| m.values())
            .filter_map(|o| o.get("create"))
            .filter_map(|c| c.get("resourceName").and_then(Value::as_str))
            .filter(|rn| rn.split('/').next_back() == Some("-2"))
            .count();
        assert_eq!(claims, 1, "only the campaign claims temp id -2");
    }

    #[test]
    fn new_labels_do_not_reclaim_resource_temp_ids() {
        // Issue #70: creating a budget + campaign together was rejected with
        // "Creating more than one resource with the same temp ID is not allowed".
        // The label counter restarted at -1, colliding with the budget's temp id
        // -1. Temp ids are unique per request across *all* resource types, so the
        // campaign's new bidsmith:address label must not reclaim -1.
        let input: ExportInput = serde_json::from_value(json!({
            "customer_id": "6571974784",
            "campaign_budgets": [{"id": "m.b", "name": "GH_Cookies", "amount_micros": 10000000}],
            "campaigns": [{
                "id": "m.c", "name": "GH_Cookies", "advertising_channel_type": "SEARCH",
                "campaign_budget": "m.b", "manual_cpc": {"enhanced_cpc_enabled": false}
            }]
        }))
        .expect("valid ExportInput");

        let report = DiffReport {
            diffs: vec![
                create_diff("m.b", "campaign_budget"),
                create_diff("m.c", "campaign"),
            ],
            label_plans: vec![LabelPlanEntry {
                address: "m.c".to_string(),
                kind: "campaign",
                label_address: "m.c".to_string(),
                existing_label_rn: None,
                stale_assoc_rn: None,
            }],
            claim_plans: Vec::new(),
            noop_count: 0,
            create_count: 2,
            update_count: 0,
            delete_count: 0,
            adopt_count: 0,
            ..DiffReport::default()
        };

        let plan = match build_mutate_with_diff(&input, &report, true) {
            Ok(plan) => plan,
            Err(errs) => panic!(
                "plan should build: {}",
                errs.iter()
                    .map(|e| format!("{}: {}", e.address, e.message))
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        };
        let ops = plan.body["mutateOperations"].as_array().unwrap();

        // Every resourceName a create pins is a temp-id claim. Collect the
        // trailing negative id of each and assert the whole request has no
        // duplicates — the exact invariant the API enforces.
        let temp_ids: Vec<i64> = ops
            .iter()
            .filter_map(|op| op.as_object())
            .flat_map(|m| m.values())
            .filter_map(|o| o.get("create"))
            .filter_map(|c| c.get("resourceName").and_then(Value::as_str))
            .filter_map(|rn| rn.rsplit('/').next())
            .filter_map(|id| id.parse::<i64>().ok())
            .filter(|id| *id < 0)
            .collect();

        let unique: HashSet<i64> = temp_ids.iter().copied().collect();
        assert_eq!(
            temp_ids.len(),
            unique.len(),
            "temp ids must be unique across the whole batch, got {temp_ids:?}"
        );

        let label_rn = ops
            .iter()
            .find_map(|o| o.get("labelOperation").and_then(|x| x.get("create")))
            .and_then(|c| c.get("resourceName"))
            .and_then(Value::as_str)
            .expect("a label create op");
        assert_eq!(
            label_rn, "customers/6571974784/labels/-3",
            "the new label continues the shared counter past the two resource creates"
        );
    }

    #[test]
    fn new_shared_set_attachments_share_one_temp_id() {
        let input: ExportInput = serde_json::from_value(json!({
            "customer_id": "6571974784",
            "shared_sets": [{
                "id": "account.google_ads_shared_set.platform_negative_keywords",
                "name": "platform campaigns negative keywords",
                "type": "NEGATIVE_KEYWORDS",
                "status": "ENABLED"
            }],
            "campaign_shared_sets": [
                {
                    "id": "chrome.google_ads_campaign_shared_set.platform_negatives",
                    "campaign": "customers/6571974784/campaigns/111",
                    "shared_set": "account.google_ads_shared_set.platform_negative_keywords"
                },
                {
                    "id": "firefox.google_ads_campaign_shared_set.platform_negatives",
                    "campaign": "customers/6571974784/campaigns/222",
                    "shared_set": "account.google_ads_shared_set.platform_negative_keywords"
                }
            ]
        }))
        .expect("valid ExportInput");

        let report = DiffReport {
            diffs: vec![
                create_diff(
                    "account.google_ads_shared_set.platform_negative_keywords",
                    "shared_set",
                ),
                create_diff(
                    "chrome.google_ads_campaign_shared_set.platform_negatives",
                    "campaign_shared_set",
                ),
                create_diff(
                    "firefox.google_ads_campaign_shared_set.platform_negatives",
                    "campaign_shared_set",
                ),
            ],
            label_plans: Vec::new(),
            claim_plans: Vec::new(),
            noop_count: 0,
            create_count: 3,
            update_count: 0,
            delete_count: 0,
            adopt_count: 0,
            ..DiffReport::default()
        };

        let plan = match build_mutate_with_diff(&input, &report, true) {
            Ok(plan) => plan,
            Err(errs) => panic!(
                "plan should build: {}",
                errs.iter()
                    .map(|e| format!("{}: {}", e.address, e.message))
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        };
        let ops = plan.body["mutateOperations"].as_array().unwrap();

        let set_create = ops
            .iter()
            .find_map(|op| op.get("sharedSetOperation").and_then(|o| o.get("create")))
            .expect("shared set create op");
        let set_rn = set_create["resourceName"].as_str().unwrap();
        assert!(
            set_rn.contains("/sharedSets/-"),
            "new set should get a temp negative resource name, got {set_rn}"
        );

        let attachments: Vec<&Value> = ops
            .iter()
            .filter_map(|op| {
                op.get("campaignSharedSetOperation")
                    .and_then(|o| o.get("create"))
            })
            .collect();
        assert_eq!(
            attachments.len(),
            2,
            "both attachments should emit a create op"
        );

        for create in attachments {
            assert!(
                create.get("resourceName").is_none(),
                "campaign shared set create must not pin a resource name that re-claims the new set's temp id"
            );
            assert_eq!(
                create["sharedSet"].as_str().unwrap(),
                set_rn,
                "every attachment must reference the one new set's temp resource name"
            );
        }
    }

    #[test]
    fn sitelink_asset_and_campaign_link_create_with_temp_id_wiring() {
        // A fresh sitelink asset + the campaign_asset that links it: the asset
        // create must precede the link create, and the link's `asset` field must
        // reference the asset op's temp resource name so the API resolves it.
        let declared: ExportInput = serde_json::from_value(json!({
            "customer_id": "100",
            "sitelink_assets": [{
                "id": "m.add_to_chrome",
                "link_text": "Add to Chrome",
                "description1": "Free, open source",
                "final_urls": ["https://ghostery.com/get"]
            }],
            "campaign_assets": [{
                "id": "m.gh_sitelink",
                "campaign": "customers/100/campaigns/555",
                "asset": "m.add_to_chrome",
                "field_type": "SITELINK",
                "status": "ENABLED"
            }]
        }))
        .expect("valid ExportInput");
        let live: ExportInput =
            serde_json::from_value(json!({ "customer_id": "100" })).expect("valid live");

        let report = crate::api::diff::diff(&declared, &live);
        let plan = match build_mutate_with_diff(&declared, &report, true) {
            Ok(plan) => plan,
            Err(errs) => panic!(
                "plan should build: {}",
                errs.iter()
                    .map(|e| format!("{}: {}", e.address, e.message))
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        };
        let ops = plan.body["mutateOperations"].as_array().unwrap();

        let asset_idx = ops
            .iter()
            .position(|o| o.get("assetOperation").is_some())
            .expect("an asset create op");
        let link_idx = ops
            .iter()
            .position(|o| o.get("campaignAssetOperation").is_some())
            .expect("a campaign asset create op");
        assert!(
            asset_idx < link_idx,
            "the referenced asset must be created before the link that uses it"
        );

        let asset = ops[asset_idx]["assetOperation"]["create"].clone();
        let asset_rn = asset["resourceName"].as_str().unwrap();
        assert!(asset_rn.starts_with("customers/100/assets/-"), "temp asset id: {asset_rn}");
        assert_eq!(
            asset["sitelinkAsset"]["linkText"].as_str().unwrap(),
            "Add to Chrome"
        );
        assert_eq!(
            asset["finalUrls"][0].as_str().unwrap(),
            "https://ghostery.com/get"
        );

        let link = ops[link_idx]["campaignAssetOperation"]["create"].clone();
        assert!(
            link.get("resourceName").is_none(),
            "campaign asset create must not pin a composite id that re-claims the new asset's temp id"
        );
        assert_eq!(link["campaign"].as_str().unwrap(), "customers/100/campaigns/555");
        assert_eq!(
            link["asset"].as_str().unwrap(),
            asset_rn,
            "the link must reference the new asset's temp resource name"
        );
        assert_eq!(link["fieldType"].as_str().unwrap(), "SITELINK");
    }

    #[test]
    fn youtube_video_asset_and_video_ad_create_with_temp_id_wiring() {
        let declared: ExportInput = serde_json::from_value(json!({
            "customer_id": "100",
            "campaign_budgets": [{ "id": "m.b", "name": "Preroll", "amount_micros": 10000000 }],
            "campaigns": [{
                "id": "m.c", "name": "Preroll", "advertising_channel_type": "VIDEO",
                "campaign_budget": "m.b"
            }],
            "ad_groups": [{ "id": "m.ag", "name": "In-stream", "campaign": "m.c" }],
            "youtube_video_assets": [{
                "id": "m.brand_12s",
                "youtube_video_id": "dQw4w9WgXcQ",
                "youtube_video_title": "Brand 12s"
            }],
            "ad_group_ads": [{
                "id": "m.preroll",
                "ad_group": "m.ag",
                "status": "PAUSED",
                "ad": {
                    "name": "Preroll 12s",
                    "final_urls": ["https://ghostery.com/get"],
                    "video_responsive_ad": {
                        "video": "m.brand_12s",
                        "headlines": ["Block ads and trackers"],
                        "long_headlines": ["Ghostery blocks ads and trackers everywhere"],
                        "descriptions": ["Install the free extension"],
                        "call_to_actions": ["Install"]
                    }
                }
            }]
        }))
        .expect("valid ExportInput");
        let live: ExportInput =
            serde_json::from_value(json!({ "customer_id": "100" })).expect("valid live");

        let report = crate::api::diff::diff(&declared, &live);
        let plan = match build_mutate_with_diff(&declared, &report, true) {
            Ok(plan) => plan,
            Err(errs) => panic!(
                "plan should build: {}",
                errs.iter()
                    .map(|e| format!("{}: {}", e.address, e.message))
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        };
        let ops = plan.body["mutateOperations"].as_array().unwrap();

        let asset_idx = ops
            .iter()
            .position(|o| o.get("assetOperation").is_some())
            .expect("a youtube video asset create op");
        let ad_idx = ops
            .iter()
            .position(|o| o.get("adGroupAdOperation").is_some())
            .expect("an ad group ad create op");
        assert!(
            asset_idx < ad_idx,
            "the video asset must be created before the ad that references it"
        );

        let asset = ops[asset_idx]["assetOperation"]["create"].clone();
        let asset_rn = asset["resourceName"].as_str().unwrap();
        assert!(asset_rn.starts_with("customers/100/assets/-"), "temp asset id: {asset_rn}");
        assert_eq!(
            asset["youtubeVideoAsset"]["youtubeVideoId"].as_str().unwrap(),
            "dQw4w9WgXcQ"
        );
        assert_eq!(
            asset["youtubeVideoAsset"]["youtubeVideoTitle"].as_str().unwrap(),
            "Brand 12s"
        );

        let ad = ops[ad_idx]["adGroupAdOperation"]["create"]["ad"]["videoResponsiveAd"].clone();
        assert_eq!(
            ad["videos"][0]["asset"].as_str().unwrap(),
            asset_rn,
            "the creative must reference the new asset's temp resource name"
        );
        assert_eq!(ad["headlines"][0]["text"].as_str().unwrap(), "Block ads and trackers");
        assert_eq!(
            ad["longHeadlines"][0]["text"].as_str().unwrap(),
            "Ghostery blocks ads and trackers everywhere"
        );
        assert_eq!(ad["descriptions"][0]["text"].as_str().unwrap(), "Install the free extension");
        assert_eq!(ad["callToActions"][0]["text"].as_str().unwrap(), "Install");
    }

    #[test]
    fn video_asset_matching_live_is_reused_rather_than_recreated() {
        let declared: ExportInput = serde_json::from_value(json!({
            "customer_id": "100",
            "campaign_budgets": [{ "id": "m.b", "name": "Preroll", "amount_micros": 10000000 }],
            "campaigns": [{
                "id": "m.c", "name": "Preroll", "advertising_channel_type": "VIDEO",
                "campaign_budget": "m.b"
            }],
            "ad_groups": [{ "id": "m.ag", "name": "In-stream", "campaign": "m.c" }],
            "youtube_video_assets": [{ "id": "m.brand_12s", "youtube_video_id": "dQw4w9WgXcQ" }],
            "ad_group_ads": [{
                "id": "m.preroll",
                "ad_group": "m.ag",
                "ad": {
                    "final_urls": ["https://ghostery.com/get"],
                    "video_responsive_ad": { "video": "m.brand_12s", "headlines": ["Block ads"] }
                }
            }]
        }))
        .expect("valid ExportInput");
        let live: ExportInput = serde_json::from_value(json!({
            "customer_id": "100",
            "youtube_video_assets": [{ "id": "42", "youtube_video_id": "dQw4w9WgXcQ" }]
        }))
        .expect("valid live");

        let report = crate::api::diff::diff(&declared, &live);
        let plan = expect_plan(build_mutate_with_diff(&declared, &report, true));
        let ops = plan.body["mutateOperations"].as_array().unwrap();

        assert!(
            !ops.iter().any(|o| o.get("assetOperation").is_some()),
            "an existing video asset must not be created again"
        );
        let ad = ops
            .iter()
            .find_map(|o| o.get("adGroupAdOperation").and_then(|o| o.get("create")))
            .expect("ad group ad create op");
        assert_eq!(
            ad["ad"]["videoResponsiveAd"]["videos"][0]["asset"].as_str().unwrap(),
            "customers/100/assets/42",
            "the creative must point at the live asset"
        );
    }

    #[test]
    fn demand_gen_video_ad_needs_business_name_and_rejects_text_ctas() {
        let declared: ExportInput = serde_json::from_value(json!({
            "customer_id": "100",
            "campaign_budgets": [{ "id": "m.b", "name": "Preroll", "amount_micros": 10000000 }],
            "campaigns": [{
                "id": "m.c", "name": "Preroll", "advertising_channel_type": "VIDEO",
                "campaign_budget": "m.b"
            }],
            "ad_groups": [{ "id": "m.ag", "name": "In-stream", "campaign": "m.c" }],
            "youtube_video_assets": [{ "id": "m.short", "youtube_video_id": "dQw4w9WgXcQ" }],
            "ad_group_ads": [{
                "id": "m.dg",
                "ad_group": "m.ag",
                "ad": {
                    "final_urls": ["https://ghostery.com/get"],
                    "demand_gen_video_responsive_ad": {
                        "videos": ["m.short"],
                        "headlines": ["Block ads"],
                        "call_to_actions": ["Install"]
                    }
                }
            }]
        }))
        .expect("valid ExportInput");
        let live: ExportInput =
            serde_json::from_value(json!({ "customer_id": "100" })).expect("valid live");

        let report = crate::api::diff::diff(&declared, &live);
        let Err(errs) = build_mutate_with_diff(&declared, &report, true) else {
            panic!("an unbuildable demand gen ad should fail the plan");
        };
        let messages = errs
            .iter()
            .map(|e| e.message.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(messages.contains("business_name"), "{messages}");
        assert!(messages.contains("CALL_TO_ACTION"), "{messages}");
    }

    #[test]
    fn demand_gen_video_ad_creates_with_business_name() {
        let declared: ExportInput = serde_json::from_value(json!({
            "customer_id": "100",
            "campaign_budgets": [{ "id": "m.b", "name": "Preroll", "amount_micros": 10000000 }],
            "campaigns": [{
                "id": "m.c", "name": "Preroll", "advertising_channel_type": "VIDEO",
                "campaign_budget": "m.b"
            }],
            "ad_groups": [{ "id": "m.ag", "name": "In-stream", "campaign": "m.c" }],
            "youtube_video_assets": [{ "id": "m.short", "youtube_video_id": "dQw4w9WgXcQ" }],
            "ad_group_ads": [{
                "id": "m.dg",
                "ad_group": "m.ag",
                "ad": {
                    "final_urls": ["https://ghostery.com/get"],
                    "demand_gen_video_responsive_ad": {
                        "videos": ["m.short"],
                        "headlines": ["Block ads"],
                        "descriptions": ["Free and open source"],
                        "business_name": "Ghostery",
                        "breadcrumb1": "Privacy"
                    }
                }
            }]
        }))
        .expect("valid ExportInput");
        let live: ExportInput =
            serde_json::from_value(json!({ "customer_id": "100" })).expect("valid live");

        let report = crate::api::diff::diff(&declared, &live);
        let plan = expect_plan(build_mutate_with_diff(&declared, &report, true));
        let ops = plan.body["mutateOperations"].as_array().unwrap();
        let dg = ops
            .iter()
            .find_map(|o| o.get("adGroupAdOperation").and_then(|o| o.get("create")))
            .expect("ad group ad create op")["ad"]["demandGenVideoResponsiveAd"]
            .clone();
        assert_eq!(dg["businessName"]["text"].as_str().unwrap(), "Ghostery");
        assert_eq!(dg["headlines"][0]["text"].as_str().unwrap(), "Block ads");
        assert_eq!(dg["breadcrumb1"].as_str().unwrap(), "Privacy");
        assert!(dg["videos"][0]["asset"].as_str().unwrap().starts_with("customers/100/assets/-"));
    }

    #[test]
    fn claim_plans_share_one_owns_label_and_release_stale_associations() {
        let input: ExportInput = serde_json::from_value(json!({
            "customer_id": "100",
            "campaigns": [{
                "id": "m.c",
                "name": "C",
                "advertising_channel_type": "SEARCH",
                "campaign_budget": "customers/100/campaignBudgets/999"
            }],
            "ad_groups": [{"id": "m.g", "name": "G", "campaign": "m.c"}]
        }))
        .expect("valid ExportInput");

        let report = DiffReport {
            diffs: vec![
                ResourceDiff {
                    address: "m.c".to_string(),
                    kind: "campaign",
                    action: Action::NoOp { live_id: "555".to_string() },
                },
                ResourceDiff {
                    address: "m.g".to_string(),
                    kind: "ad_group",
                    action: Action::NoOp { live_id: "300".to_string() },
                },
            ],
            label_plans: Vec::new(),
            claim_plans: vec![
                ClaimPlanEntry {
                    address: "m.c".to_string(),
                    kind: "campaign",
                    category: "keyword_negative",
                    existing_label_rn: None,
                    stale_assoc_rn: None,
                },
                ClaimPlanEntry {
                    address: "m.g".to_string(),
                    kind: "ad_group",
                    category: "keyword_negative",
                    existing_label_rn: None,
                    stale_assoc_rn: None,
                },
                ClaimPlanEntry {
                    address: "m.g".to_string(),
                    kind: "ad_group",
                    category: "location",
                    existing_label_rn: None,
                    stale_assoc_rn: Some("customers/100/adGroupLabels/300~779".to_string()),
                },
            ],
            noop_count: 2,
            create_count: 0,
            update_count: 0,
            delete_count: 0,
            adopt_count: 2,
            ..DiffReport::default()
        };

        let plan = match build_mutate_with_diff(&input, &report, true) {
            Ok(plan) => plan,
            Err(errs) => panic!(
                "plan should build: {}",
                errs.iter()
                    .map(|e| format!("{}: {}", e.address, e.message))
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        };
        let ops = plan.body["mutateOperations"].as_array().unwrap();

        let label_creates: Vec<&Value> = ops
            .iter()
            .filter_map(|o| o.get("labelOperation").and_then(|x| x.get("create")))
            .collect();
        assert_eq!(
            label_creates.len(),
            1,
            "two claims of the same category must mint one label: {ops:?}"
        );
        assert_eq!(
            label_creates[0]["name"].as_str().unwrap(),
            "bidsmith:owns=keyword_negative"
        );
        let label_rn = label_creates[0]["resourceName"].as_str().unwrap();

        let campaign_assoc = ops
            .iter()
            .find_map(|o| o.get("campaignLabelOperation").and_then(|x| x.get("create")))
            .expect("a campaign claim association");
        assert_eq!(
            campaign_assoc["campaign"].as_str().unwrap(),
            "customers/100/campaigns/555"
        );
        assert_eq!(campaign_assoc["label"].as_str().unwrap(), label_rn);

        let ag_assoc = ops
            .iter()
            .find_map(|o| o.get("adGroupLabelOperation").and_then(|x| x.get("create")))
            .expect("an ad group claim association");
        assert_eq!(ag_assoc["adGroup"].as_str().unwrap(), "customers/100/adGroups/300");
        assert_eq!(ag_assoc["label"].as_str().unwrap(), label_rn);

        let release = ops
            .iter()
            .find_map(|o| o.get("adGroupLabelOperation").and_then(|x| x.get("remove")))
            .expect("a claim release");
        assert_eq!(release.as_str().unwrap(), "customers/100/adGroupLabels/300~779");
    }
}
