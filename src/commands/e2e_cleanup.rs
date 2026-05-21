use std::process::ExitCode;

use serde_json::Value;

use crate::api::{auth, client, mutate};

const CLEANUP_QUERIES: &[(&str, &str)] = &[
    (
        "ad_group_criterion",
        "SELECT ad_group_criterion.resource_name
         FROM ad_group_criterion
         WHERE ad_group.name LIKE '{prefix}%'
           AND ad_group_criterion.status != 'REMOVED'",
    ),
    (
        "campaign_criterion",
        "SELECT campaign_criterion.resource_name
         FROM campaign_criterion
         WHERE campaign.name LIKE '{prefix}%'
           AND campaign_criterion.status != 'REMOVED'",
    ),
    (
        "ad_group_ad",
        "SELECT ad_group_ad.resource_name
         FROM ad_group_ad
         WHERE ad_group.name LIKE '{prefix}%'
           AND ad_group_ad.status != 'REMOVED'",
    ),
    (
        "ad_group",
        "SELECT ad_group.resource_name
         FROM ad_group
         WHERE ad_group.name LIKE '{prefix}%'
           AND ad_group.status != 'REMOVED'",
    ),
    (
        "campaign",
        "SELECT campaign.resource_name
         FROM campaign
         WHERE campaign.name LIKE '{prefix}%'
           AND campaign.status != 'REMOVED'",
    ),
    (
        "campaign_budget",
        "SELECT campaign_budget.resource_name
         FROM campaign_budget
         WHERE campaign_budget.name LIKE '{prefix}%'",
    ),
];

pub fn run(prefix: &str, verbose: bool) -> ExitCode {
    if !is_safe_prefix(prefix) {
        eprintln!(
            "_e2e-cleanup: refusing prefix `{prefix}` — must be non-empty ASCII \
             alphanumerics or `-` only (no quotes, `%`, `_`, etc.)",
        );
        return ExitCode::from(2);
    }

    let client = match client::Client::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("_e2e-cleanup: {e}");
            return ExitCode::from(1);
        }
    };
    let token = match auth::get_access_token() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("_e2e-cleanup: {e}");
            return ExitCode::from(1);
        }
    };

    let mut targets: Vec<(&'static str, String)> = Vec::new();
    for &(kind, query_template) in CLEANUP_QUERIES {
        let gaql = query_template.replace("{prefix}", prefix);
        if verbose {
            eprintln!("_e2e-cleanup: query[{kind}]");
        }
        let response = match client.search_stream(&token.token, &gaql) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("_e2e-cleanup: search_stream failed for {kind}: {e}");
                return ExitCode::from(1);
            }
        };
        if response.status < 200 || response.status >= 300 {
            eprintln!(
                "_e2e-cleanup: HTTP {} querying {kind}: {}",
                response.status, response.body_raw,
            );
            return ExitCode::from(1);
        }
        let names = extract_resource_names(kind, &response.body);
        if verbose {
            eprintln!("_e2e-cleanup:   {} matching", names.len());
        }
        for rn in names {
            targets.push((kind, rn));
        }
    }

    if targets.is_empty() {
        eprintln!(
            "_e2e-cleanup: nothing to remove (prefix `{prefix}` matched 0 resources).",
        );
        return ExitCode::SUCCESS;
    }

    eprintln!(
        "_e2e-cleanup: removing {} resource(s) with prefix `{prefix}`...",
        targets.len(),
    );
    if verbose {
        for (kind, rn) in &targets {
            eprintln!("  - {kind}: {rn}");
        }
    }

    let body = mutate::build_remove_only_mutate(&targets, false);
    let response = match client.googleads_mutate(&token.token, &body) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("_e2e-cleanup: mutate failed: {e}");
            return ExitCode::from(1);
        }
    };

    if response.status < 200 || response.status >= 300 {
        eprintln!(
            "_e2e-cleanup: HTTP {} on REMOVE batch:\n{}",
            response.status, response.body_raw,
        );
        return ExitCode::from(1);
    }

    println!("_e2e-cleanup: removed {} resource(s).", targets.len());
    ExitCode::SUCCESS
}

fn is_safe_prefix(prefix: &str) -> bool {
    !prefix.is_empty()
        && prefix
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn extract_resource_names(kind: &str, body: &Value) -> Vec<String> {
    let row_key = match kind {
        "ad_group_criterion" => "adGroupCriterion",
        "campaign_criterion" => "campaignCriterion",
        "ad_group_ad" => "adGroupAd",
        "ad_group" => "adGroup",
        "campaign" => "campaign",
        "campaign_budget" => "campaignBudget",
        _ => return Vec::new(),
    };

    let batches: Vec<&Value> = match body.as_array() {
        Some(arr) => arr.iter().collect(),
        None => vec![body],
    };

    let mut out: Vec<String> = Vec::new();
    for batch in batches {
        let Some(results) = batch.get("results").and_then(Value::as_array) else {
            continue;
        };
        for row in results {
            if let Some(rn) = row
                .get(row_key)
                .and_then(|v| v.get("resourceName"))
                .and_then(Value::as_str)
            {
                out.push(rn.to_string());
            }
        }
    }
    out
}
