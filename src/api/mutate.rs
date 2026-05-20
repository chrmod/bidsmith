use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value, json};

use crate::api::diff::{Action, DiffReport};
use crate::commands::export::{
    ExportInput, JsonAd, JsonAdGroup, JsonAdGroupAd, JsonAdGroupCriterion, JsonBudget,
    JsonCampaign, JsonCampaignCriterion, JsonResponsiveSearchAd, JsonRsaAsset,
};

pub struct PlanOperation {
    pub address: String,
    #[allow(dead_code)]
    pub kind: &'static str,
}

pub struct PlanBody {
    pub body: Value,
    pub operations: Vec<PlanOperation>,
}

pub struct PlanBuildError {
    pub address: String,
    pub message: String,
}

pub fn build_mutate_with_diff(
    input: &ExportInput,
    report: &DiffReport,
    validate_only: bool,
) -> Result<PlanBody, Vec<PlanBuildError>> {
    let customer_id = input.customer_id.as_str();
    let mut refs: HashMap<String, String> = HashMap::new();
    let mut next: i32 = -1;
    let mut create_set: HashSet<String> = HashSet::new();

    let mut update_set: HashMap<String, &[String]> = HashMap::new();

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
            _ => continue,
        };
        if let Some(live_id) = d.action.live_id() {
            refs.insert(
                d.address.clone(),
                format!("customers/{customer_id}/{segment}/{live_id}"),
            );
            if let Action::Update { changed_fields, .. } = &d.action {
                update_set.insert(d.address.clone(), changed_fields.as_slice());
            }
        } else {
            create_set.insert(d.address.clone());
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
                "campaign_criterion" => {
                    let cr = input.campaign_criteria.iter().find(|c| c.id == d.address);
                    let criterion_segment =
                        if let Some(loc) = cr.and_then(|c| c.location.as_ref()) {
                            last_segment(&loc.geo_target_constant).to_string()
                        } else if let Some(lang) = cr.and_then(|c| c.language.as_ref()) {
                            last_segment(&lang.language_constant).to_string()
                        } else {
                            next.to_string()
                        };
                    let parent_addr = cr.map(|c| c.campaign.as_str()).unwrap_or("");
                    let parent_id = refs
                        .get(parent_addr)
                        .and_then(|rn| rn.rsplit('/').next())
                        .unwrap_or("0");
                    format!(
                        "customers/{customer_id}/{segment}/{parent_id}~{criterion_segment}"
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

    for b in &input.campaign_budgets {
        let rn = refs.get(&b.id).expect("budget rn");
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
        let rn = refs.get(&c.id).expect("campaign rn");
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
                    "updateMask": fields.join(","),
                }
            }));
            operations.push(PlanOperation { address: c.id.clone(), kind: "campaign" });
        }
    }
    for g in &input.ad_groups {
        let rn = refs.get(&g.id).expect("ad_group rn");
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
    for a in &input.ad_group_ads {
        let rn = refs.get(&a.id).expect("ad_group_ad rn");
        if create_set.contains(&a.id) {
            let ag_rn = match resolve(&refs, &a.ad_group, &a.id, "ad_group", &mut errors) {
                Some(s) => s,
                None => continue,
            };
            mutate_ops.push(json!({
                "adGroupAdOperation": { "create": ad_group_ad_create(a, rn, &ag_rn) }
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
        let rn = refs.get(&cr.id).expect("ad_group_criterion rn");
        if create_set.contains(&cr.id) {
            let ag_rn = match resolve(&refs, &cr.ad_group, &cr.id, "ad_group", &mut errors) {
                Some(s) => s,
                None => continue,
            };
            mutate_ops.push(json!({
                "adGroupCriterionOperation": {
                    "create": ad_group_criterion_create(cr, rn, &ag_rn)
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
        let rn = refs.get(&cr.id).expect("campaign_criterion rn");
        if create_set.contains(&cr.id) {
            let camp_rn = match resolve(&refs, &cr.campaign, &cr.id, "campaign", &mut errors) {
                Some(s) => s,
                None => continue,
            };
            mutate_ops.push(json!({
                "campaignCriterionOperation": {
                    "create": campaign_criterion_create(cr, rn, &camp_rn)
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

    if !errors.is_empty() {
        return Err(errors);
    }

    let body = json!({
        "mutateOperations": mutate_ops,
        "validateOnly": validate_only,
    });
    Ok(PlanBody { body, operations })
}

#[allow(dead_code)]
pub fn build_validate_only(input: &ExportInput) -> Result<PlanBody, Vec<PlanBuildError>> {
    let customer_id = input.customer_id.as_str();
    let mut refs: HashMap<String, String> = HashMap::new();
    let mut next: i32 = -1;

    // Pass 1: assign temp resourceNames to every resource, keyed by .bid address.
    for b in &input.campaign_budgets {
        refs.insert(b.id.clone(), temp_rn(customer_id, "campaignBudgets", next));
        next -= 1;
    }
    for c in &input.campaigns {
        refs.insert(c.id.clone(), temp_rn(customer_id, "campaigns", next));
        next -= 1;
    }
    for g in &input.ad_groups {
        refs.insert(g.id.clone(), temp_rn(customer_id, "adGroups", next));
        next -= 1;
    }
    for a in &input.ad_group_ads {
        let rn = composite_rn(customer_id, "adGroupAds", &refs, &a.ad_group, next);
        refs.insert(a.id.clone(), rn);
        next -= 1;
    }
    for cr in &input.ad_group_criteria {
        let rn = composite_rn(customer_id, "adGroupCriteria", &refs, &cr.ad_group, next);
        refs.insert(cr.id.clone(), rn);
        next -= 1;
    }
    for cr in &input.campaign_criteria {
        // For LOCATION and LANGUAGE criteria, the second half of the resource
        // name is the constant's own id (not a temp negative) — Google
        // cross-checks against the location.geoTargetConstant / language
        // fields, so the two must agree.
        let criterion_segment = if let Some(loc) = &cr.location {
            last_segment(&loc.geo_target_constant).to_string()
        } else if let Some(lang) = &cr.language {
            last_segment(&lang.language_constant).to_string()
        } else {
            let s = next.to_string();
            next -= 1;
            s
        };
        let parent_id = refs
            .get(&cr.campaign)
            .and_then(|rn| rn.rsplit('/').next())
            .unwrap_or("0");
        let rn = format!(
            "customers/{customer_id}/campaignCriteria/{parent_id}~{criterion_segment}"
        );
        refs.insert(cr.id.clone(), rn);
    }

    // Pass 2: emit operations in dependency order, resolving refs.
    let mut errors: Vec<PlanBuildError> = Vec::new();
    let mut operations: Vec<PlanOperation> = Vec::new();
    let mut mutate_ops: Vec<Value> = Vec::new();

    for b in &input.campaign_budgets {
        let rn = refs.get(&b.id).expect("budget rn");
        mutate_ops.push(json!({ "campaignBudgetOperation": { "create": budget_create(b, rn) } }));
        operations.push(PlanOperation { address: b.id.clone(), kind: "campaign_budget" });
    }
    for c in &input.campaigns {
        let rn = refs.get(&c.id).expect("campaign rn");
        let budget_rn = match resolve(&refs, &c.campaign_budget, &c.id, "campaign_budget", &mut errors) {
            Some(s) => s,
            None => continue,
        };
        mutate_ops.push(json!({ "campaignOperation": { "create": campaign_create(c, rn, &budget_rn) } }));
        operations.push(PlanOperation { address: c.id.clone(), kind: "campaign" });
    }
    for g in &input.ad_groups {
        let rn = refs.get(&g.id).expect("ad_group rn");
        let campaign_rn = match resolve(&refs, &g.campaign, &g.id, "campaign", &mut errors) {
            Some(s) => s,
            None => continue,
        };
        mutate_ops.push(json!({ "adGroupOperation": { "create": ad_group_create(g, rn, &campaign_rn) } }));
        operations.push(PlanOperation { address: g.id.clone(), kind: "ad_group" });
    }
    for a in &input.ad_group_ads {
        let rn = refs.get(&a.id).expect("ad_group_ad rn");
        let ag_rn = match resolve(&refs, &a.ad_group, &a.id, "ad_group", &mut errors) {
            Some(s) => s,
            None => continue,
        };
        mutate_ops.push(json!({ "adGroupAdOperation": { "create": ad_group_ad_create(a, rn, &ag_rn) } }));
        operations.push(PlanOperation { address: a.id.clone(), kind: "ad_group_ad" });
    }
    for cr in &input.ad_group_criteria {
        let rn = refs.get(&cr.id).expect("ad_group_criterion rn");
        let ag_rn = match resolve(&refs, &cr.ad_group, &cr.id, "ad_group", &mut errors) {
            Some(s) => s,
            None => continue,
        };
        mutate_ops.push(json!({
            "adGroupCriterionOperation": {
                "create": ad_group_criterion_create(cr, rn, &ag_rn)
            }
        }));
        operations.push(PlanOperation { address: cr.id.clone(), kind: "ad_group_criterion" });
    }
    for cr in &input.campaign_criteria {
        let rn = refs.get(&cr.id).expect("campaign_criterion rn");
        let camp_rn = match resolve(&refs, &cr.campaign, &cr.id, "campaign", &mut errors) {
            Some(s) => s,
            None => continue,
        };
        mutate_ops.push(json!({
            "campaignCriterionOperation": {
                "create": campaign_criterion_create(cr, rn, &camp_rn)
            }
        }));
        operations.push(PlanOperation { address: cr.id.clone(), kind: "campaign_criterion" });
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    let body = json!({
        "mutateOperations": mutate_ops,
        "validateOnly": true,
    });
    Ok(PlanBody { body, operations })
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

fn last_segment(s: &str) -> &str {
    s.rsplit('/').next().unwrap_or(s)
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

fn campaign_update_body(c: &JsonCampaign, resource_name: &str, fields: &[String]) -> Value {
    let mut m = Map::new();
    m.insert("resourceName".into(), Value::String(resource_name.to_string()));
    let mut manual_cpc_sub: Option<Map<String, Value>> = None;
    let mut network_sub: Option<Map<String, Value>> = None;
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
            "manual_cpc.enhanced_cpc_enabled" => {
                let sub = manual_cpc_sub.get_or_insert_with(Map::new);
                if let Some(e) = c.manual_cpc.as_ref().and_then(|m| m.enhanced_cpc_enabled) {
                    sub.insert("enhancedCpcEnabled".into(), Value::Bool(e));
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
            _ => {}
        }
    }
    if let Some(sub) = manual_cpc_sub {
        m.insert("manualCpc".into(), Value::Object(sub));
    }
    if let Some(sub) = network_sub {
        m.insert("networkSettings".into(), Value::Object(sub));
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
            "cpc_bid_micros" => {
                if let Some(c) = g.cpc_bid_micros {
                    m.insert("cpcBidMicros".into(), Value::String(c.to_string()));
                }
            }
            _ => {}
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
            _ => {}
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
    m.insert(
        "containsEuPoliticalAdvertising".into(),
        Value::String("DOES_NOT_CONTAIN_EU_POLITICAL_ADVERTISING".to_string()),
    );
    if let Some(mc) = &c.manual_cpc {
        let mut sub = Map::new();
        if let Some(e) = mc.enhanced_cpc_enabled {
            sub.insert("enhancedCpcEnabled".into(), Value::Bool(e));
        }
        m.insert("manualCpc".into(), Value::Object(sub));
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
    Value::Object(m)
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
    if let Some(c) = g.cpc_bid_micros {
        m.insert("cpcBidMicros".into(), Value::String(c.to_string()));
    }
    Value::Object(m)
}

fn ad_group_ad_create(a: &JsonAdGroupAd, resource_name: &str, ag_rn: &str) -> Value {
    let mut m = Map::new();
    m.insert("resourceName".into(), Value::String(resource_name.to_string()));
    m.insert("adGroup".into(), Value::String(ag_rn.to_string()));
    if let Some(s) = &a.status {
        m.insert("status".into(), Value::String(s.clone()));
    }
    m.insert("ad".into(), ad_value(&a.ad));
    Value::Object(m)
}

fn ad_value(ad: &JsonAd) -> Value {
    let mut m = Map::new();
    if let Some(n) = &ad.name {
        m.insert("name".into(), Value::String(n.clone()));
    }
    let urls: Vec<Value> = ad.final_urls.iter().cloned().map(Value::String).collect();
    m.insert("finalUrls".into(), Value::Array(urls));
    if let Some(rsa) = &ad.responsive_search_ad {
        m.insert("responsiveSearchAd".into(), rsa_value(rsa));
    }
    Value::Object(m)
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
    let mut kw = Map::new();
    kw.insert("text".into(), Value::String(cr.keyword.text.clone()));
    kw.insert("matchType".into(), Value::String(cr.keyword.match_type.clone()));
    m.insert("keyword".into(), Value::Object(kw));
    Value::Object(m)
}

fn campaign_criterion_create(
    cr: &JsonCampaignCriterion,
    resource_name: &str,
    camp_rn: &str,
) -> Value {
    let mut m = Map::new();
    m.insert("resourceName".into(), Value::String(resource_name.to_string()));
    m.insert("campaign".into(), Value::String(camp_rn.to_string()));
    if let Some(s) = &cr.status {
        m.insert("status".into(), Value::String(s.clone()));
    }
    if let Some(n) = cr.negative {
        m.insert("negative".into(), Value::Bool(n));
    }
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
    Value::Object(m)
}
