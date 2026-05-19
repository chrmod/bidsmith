use hcl_edit::Span;
use hcl_edit::expr::{Expression, Traversal, TraversalOperator};
use hcl_edit::structure::{Attribute, Block, Structure};

use crate::commands::export::{
    ExportInput, JsonAd, JsonAdGroup, JsonAdGroupAd, JsonAdGroupCriterion, JsonBudget,
    JsonCampaign, JsonCampaignCriterion, JsonGeoPoint, JsonKeyword, JsonLanguage, JsonLocation,
    JsonManualCpc, JsonNetworkSettings, JsonProximity, JsonResponsiveSearchAd, JsonRsaAsset,
};
use crate::diagnostics::Diag;
use crate::parser::ParsedFile;

pub struct ImportResult {
    pub input: ExportInput,
    pub skipped: Vec<(String, String)>,
}

pub fn import_files(files: &[ParsedFile]) -> Result<ImportResult, Vec<Diag>> {
    let mut diags = Vec::new();
    let mut input = ExportInput {
        customer_id: String::new(),
        login_customer_id: None,
        campaign_budgets: Vec::new(),
        campaigns: Vec::new(),
        ad_groups: Vec::new(),
        ad_group_ads: Vec::new(),
        ad_group_criteria: Vec::new(),
        campaign_criteria: Vec::new(),
    };
    let mut skipped: Vec<(String, String)> = Vec::new();

    for f in files {
        for s in f.body.iter() {
            let Structure::Block(b) = s else { continue };
            match b.ident.as_str() {
                "provider" => import_provider(f, b, &mut input, &mut diags),
                "resource" => {
                    if b.labels.len() != 2 {
                        continue;
                    }
                    let ty = b.labels[0].as_str();
                    let name = b.labels[1].as_str();
                    let address = format!("{ty}.{name}");
                    let mut emit = |result: Result<(), Diag>| {
                        if let Err(d) = result {
                            diags.push(d);
                        }
                    };
                    match ty {
                        "google_ads_campaign_budget" => emit(
                            import_budget(f, b, &address).map(|x| input.campaign_budgets.push(x)),
                        ),
                        "google_ads_campaign" => emit(
                            import_campaign(f, b, &address).map(|x| input.campaigns.push(x)),
                        ),
                        "google_ads_ad_group" => emit(
                            import_ad_group(f, b, &address).map(|x| input.ad_groups.push(x)),
                        ),
                        "google_ads_ad_group_ad" => emit(
                            import_ad_group_ad(f, b, &address).map(|x| input.ad_group_ads.push(x)),
                        ),
                        "google_ads_ad_group_criterion" => emit(
                            import_ad_group_criterion(f, b, &address)
                                .map(|x| input.ad_group_criteria.push(x)),
                        ),
                        "google_ads_campaign_criterion" => emit(
                            import_campaign_criterion(f, b, &address)
                                .map(|x| input.campaign_criteria.push(x)),
                        ),
                        other => {
                            skipped.push((address, other.to_string()));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    if input.customer_id.is_empty() {
        if let Some(env_id) = std::env::var("GOOGLE_ADS_CUSTOMER_ID")
            .ok()
            .filter(|s| !s.is_empty())
        {
            input.customer_id = env_id;
        }
    }
    if input.login_customer_id.as_deref().unwrap_or("").is_empty() {
        if let Some(env_id) = std::env::var("GOOGLE_ADS_LOGIN_CUSTOMER_ID")
            .ok()
            .filter(|s| !s.is_empty())
        {
            input.login_customer_id = Some(env_id);
        }
    }

    if !diags.is_empty() {
        return Err(diags);
    }
    Ok(ImportResult { input, skipped })
}

fn import_provider(
    file: &ParsedFile,
    block: &Block,
    input: &mut ExportInput,
    diags: &mut Vec<Diag>,
) {
    if block.labels.len() != 1 || block.labels[0].as_str() != "google_ads" {
        return;
    }
    for s in block.body.iter() {
        let Structure::Attribute(a) = s else { continue };
        match a.key.as_str() {
            "customer_id" => {
                if let Some(v) = expect_string(file, a, diags) {
                    input.customer_id = v;
                }
            }
            "login_customer_id" => {
                if let Some(v) = expect_string(file, a, diags) {
                    input.login_customer_id = Some(v);
                }
            }
            _ => {}
        }
    }
}

fn import_budget(file: &ParsedFile, block: &Block, address: &str) -> Result<JsonBudget, Diag> {
    let mut name = None;
    let mut amount = None;
    let mut delivery_method = None;
    let mut explicitly_shared = None;

    for s in block.body.iter() {
        let Structure::Attribute(a) = s else { continue };
        match a.key.as_str() {
            "name" => name = expect_string_owned(a),
            "amount_micros" => amount = expect_i64(a),
            "delivery_method" => delivery_method = expect_string_owned(a),
            "explicitly_shared" => explicitly_shared = expect_bool(a),
            _ => {}
        }
    }

    let name = name.ok_or_else(|| missing(file, block, address, "name"))?;
    let amount = amount.ok_or_else(|| missing(file, block, address, "amount_micros"))?;
    Ok(JsonBudget {
        id: address.to_string(),
        name,
        amount_micros: amount,
        delivery_method,
        explicitly_shared,
    })
}

fn import_campaign(file: &ParsedFile, block: &Block, address: &str) -> Result<JsonCampaign, Diag> {
    let mut name = None;
    let mut status = None;
    let mut channel = None;
    let mut budget_ref = None;
    let mut manual_cpc = None;
    let mut network_settings = None;

    for s in block.body.iter() {
        match s {
            Structure::Attribute(a) => match a.key.as_str() {
                "name" => name = expect_string_owned(a),
                "status" => status = expect_string_owned(a),
                "advertising_channel_type" => channel = expect_string_owned(a),
                "campaign_budget" => budget_ref = extract_resource_ref(&a.value),
                _ => {}
            },
            Structure::Block(b) => match b.ident.as_str() {
                "manual_cpc" => manual_cpc = Some(import_manual_cpc(b)),
                "network_settings" => network_settings = Some(import_network_settings(b)),
                _ => {}
            },
        }
    }

    let name = name.ok_or_else(|| missing(file, block, address, "name"))?;
    let channel = channel.ok_or_else(|| missing(file, block, address, "advertising_channel_type"))?;
    let budget = budget_ref.ok_or_else(|| missing(file, block, address, "campaign_budget"))?;

    Ok(JsonCampaign {
        id: address.to_string(),
        name,
        status,
        advertising_channel_type: channel,
        campaign_budget: budget,
        manual_cpc,
        network_settings,
    })
}

fn import_manual_cpc(block: &Block) -> JsonManualCpc {
    let mut enhanced = None;
    for s in block.body.iter() {
        if let Structure::Attribute(a) = s {
            if a.key.as_str() == "enhanced_cpc_enabled" {
                enhanced = expect_bool(a);
            }
        }
    }
    JsonManualCpc {
        enhanced_cpc_enabled: enhanced,
    }
}

fn import_network_settings(block: &Block) -> JsonNetworkSettings {
    let mut s = JsonNetworkSettings {
        target_google_search: None,
        target_search_network: None,
        target_content_network: None,
        target_partner_search_network: None,
    };
    for st in block.body.iter() {
        if let Structure::Attribute(a) = st {
            match a.key.as_str() {
                "target_google_search" => s.target_google_search = expect_bool(a),
                "target_search_network" => s.target_search_network = expect_bool(a),
                "target_content_network" => s.target_content_network = expect_bool(a),
                "target_partner_search_network" => s.target_partner_search_network = expect_bool(a),
                _ => {}
            }
        }
    }
    s
}

fn import_ad_group(file: &ParsedFile, block: &Block, address: &str) -> Result<JsonAdGroup, Diag> {
    let mut name = None;
    let mut campaign_ref = None;
    let mut status = None;
    let mut ty = None;
    let mut cpc = None;

    for s in block.body.iter() {
        let Structure::Attribute(a) = s else { continue };
        match a.key.as_str() {
            "name" => name = expect_string_owned(a),
            "campaign" => campaign_ref = extract_resource_ref(&a.value),
            "status" => status = expect_string_owned(a),
            "type" => ty = expect_string_owned(a),
            "cpc_bid_micros" => cpc = expect_i64(a),
            _ => {}
        }
    }

    let name = name.ok_or_else(|| missing(file, block, address, "name"))?;
    let campaign = campaign_ref.ok_or_else(|| missing(file, block, address, "campaign"))?;
    Ok(JsonAdGroup {
        id: address.to_string(),
        name,
        campaign,
        status,
        ty,
        cpc_bid_micros: cpc,
    })
}

fn import_ad_group_ad(
    file: &ParsedFile,
    block: &Block,
    address: &str,
) -> Result<JsonAdGroupAd, Diag> {
    let mut ad_group_ref = None;
    let mut status = None;
    let mut ad = None;

    for s in block.body.iter() {
        match s {
            Structure::Attribute(a) => match a.key.as_str() {
                "ad_group" => ad_group_ref = extract_resource_ref(&a.value),
                "status" => status = expect_string_owned(a),
                _ => {}
            },
            Structure::Block(b) if b.ident.as_str() == "ad" => {
                ad = Some(import_ad(b));
            }
            _ => {}
        }
    }

    let ad_group = ad_group_ref.ok_or_else(|| missing(file, block, address, "ad_group"))?;
    let ad = ad.ok_or_else(|| missing(file, block, address, "ad"))?;
    Ok(JsonAdGroupAd {
        id: address.to_string(),
        ad_group,
        status,
        ad,
    })
}

fn import_ad(block: &Block) -> JsonAd {
    let mut name = None;
    let mut final_urls: Vec<String> = Vec::new();
    let mut rsa = None;

    for s in block.body.iter() {
        match s {
            Structure::Attribute(a) => match a.key.as_str() {
                "name" => name = expect_string_owned(a),
                "final_urls" => final_urls = expect_string_list(&a.value),
                _ => {}
            },
            Structure::Block(b) if b.ident.as_str() == "responsive_search_ad" => {
                rsa = Some(import_rsa(b));
            }
            _ => {}
        }
    }

    JsonAd {
        name,
        final_urls,
        responsive_search_ad: rsa,
    }
}

fn import_rsa(block: &Block) -> JsonResponsiveSearchAd {
    let mut path1 = None;
    let mut path2 = None;
    let mut headlines: Vec<JsonRsaAsset> = Vec::new();
    let mut descriptions: Vec<JsonRsaAsset> = Vec::new();

    for s in block.body.iter() {
        match s {
            Structure::Attribute(a) => match a.key.as_str() {
                "path1" => path1 = expect_string_owned(a),
                "path2" => path2 = expect_string_owned(a),
                _ => {}
            },
            Structure::Block(b) => match b.ident.as_str() {
                "headline" => {
                    if let Some(asset) = import_rsa_asset(b) {
                        headlines.push(asset);
                    }
                }
                "description" => {
                    if let Some(asset) = import_rsa_asset(b) {
                        descriptions.push(asset);
                    }
                }
                _ => {}
            },
        }
    }

    JsonResponsiveSearchAd {
        headlines,
        descriptions,
        path1,
        path2,
    }
}

fn import_rsa_asset(block: &Block) -> Option<JsonRsaAsset> {
    let mut text = None;
    let mut pin = None;
    for s in block.body.iter() {
        if let Structure::Attribute(a) = s {
            match a.key.as_str() {
                "text" => text = expect_string_owned(a),
                "pin" => pin = expect_string_owned(a),
                _ => {}
            }
        }
    }
    Some(JsonRsaAsset { text: text?, pin })
}

fn import_ad_group_criterion(
    file: &ParsedFile,
    block: &Block,
    address: &str,
) -> Result<JsonAdGroupCriterion, Diag> {
    let mut ad_group_ref = None;
    let mut status = None;
    let mut negative = None;
    let mut cpc = None;
    let mut keyword = None;

    for s in block.body.iter() {
        match s {
            Structure::Attribute(a) => match a.key.as_str() {
                "ad_group" => ad_group_ref = extract_resource_ref(&a.value),
                "status" => status = expect_string_owned(a),
                "negative" => negative = expect_bool(a),
                "cpc_bid_micros" => cpc = expect_i64(a),
                _ => {}
            },
            Structure::Block(b) if b.ident.as_str() == "keyword" => {
                keyword = import_keyword(b);
            }
            _ => {}
        }
    }

    let ad_group = ad_group_ref.ok_or_else(|| missing(file, block, address, "ad_group"))?;
    let keyword = keyword.ok_or_else(|| missing(file, block, address, "keyword"))?;
    Ok(JsonAdGroupCriterion {
        id: address.to_string(),
        ad_group,
        status,
        negative,
        cpc_bid_micros: cpc,
        keyword,
    })
}

fn import_campaign_criterion(
    file: &ParsedFile,
    block: &Block,
    address: &str,
) -> Result<JsonCampaignCriterion, Diag> {
    let mut campaign_ref = None;
    let mut status = None;
    let mut negative = None;
    let mut keyword = None;
    let mut location = None;
    let mut language = None;
    let mut proximity = None;

    for s in block.body.iter() {
        match s {
            Structure::Attribute(a) => match a.key.as_str() {
                "campaign" => campaign_ref = extract_resource_ref(&a.value),
                "status" => status = expect_string_owned(a),
                "negative" => negative = expect_bool(a),
                _ => {}
            },
            Structure::Block(b) => match b.ident.as_str() {
                "keyword" => keyword = import_keyword(b),
                "location" => location = import_location(b),
                "language" => language = import_language(b),
                "proximity" => proximity = import_proximity(b),
                _ => {}
            },
        }
    }

    let campaign = campaign_ref.ok_or_else(|| missing(file, block, address, "campaign"))?;
    Ok(JsonCampaignCriterion {
        id: address.to_string(),
        campaign,
        status,
        negative,
        keyword,
        location,
        language,
        proximity,
    })
}

fn import_keyword(block: &Block) -> Option<JsonKeyword> {
    let mut text = None;
    let mut match_type = None;
    for s in block.body.iter() {
        if let Structure::Attribute(a) = s {
            match a.key.as_str() {
                "text" => text = expect_string_owned(a),
                "match_type" => match_type = expect_string_owned(a),
                _ => {}
            }
        }
    }
    Some(JsonKeyword {
        text: text?,
        match_type: match_type?,
    })
}

fn import_location(block: &Block) -> Option<JsonLocation> {
    let mut geo = None;
    for s in block.body.iter() {
        if let Structure::Attribute(a) = s {
            if a.key.as_str() == "geo_target_constant" {
                geo = expect_string_owned(a);
            }
        }
    }
    Some(JsonLocation {
        geo_target_constant: geo?,
    })
}

fn import_language(block: &Block) -> Option<JsonLanguage> {
    let mut lang = None;
    for s in block.body.iter() {
        if let Structure::Attribute(a) = s {
            if a.key.as_str() == "language_constant" {
                lang = expect_string_owned(a);
            }
        }
    }
    Some(JsonLanguage {
        language_constant: lang?,
    })
}

fn import_proximity(block: &Block) -> Option<JsonProximity> {
    let mut radius = None;
    let mut units = None;
    let mut lat = None;
    let mut lng = None;
    for s in block.body.iter() {
        match s {
            Structure::Attribute(a) => match a.key.as_str() {
                "radius" => radius = expect_f64(a),
                "radius_units" => units = expect_string_owned(a),
                _ => {}
            },
            Structure::Block(b) if b.ident.as_str() == "geo_point" => {
                for s in b.body.iter() {
                    if let Structure::Attribute(a) = s {
                        match a.key.as_str() {
                            "latitude_in_micro_degrees" => lat = expect_i64(a),
                            "longitude_in_micro_degrees" => lng = expect_i64(a),
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Some(JsonProximity {
        radius: radius?,
        radius_units: units?,
        geo_point: JsonGeoPoint {
            latitude_in_micro_degrees: lat?,
            longitude_in_micro_degrees: lng?,
        },
    })
}

fn expect_string(file: &ParsedFile, attr: &Attribute, diags: &mut Vec<Diag>) -> Option<String> {
    if let Expression::String(s) = &attr.value {
        Some(s.as_str().to_string())
    } else {
        diags.push(Diag::new(
            file.src.clone(),
            span_of(attr.key.span()),
            format!("expected string value for '{}'", attr.key.as_str()),
        ));
        None
    }
}

fn expect_string_owned(attr: &Attribute) -> Option<String> {
    if let Expression::String(s) = &attr.value {
        Some(s.as_str().to_string())
    } else {
        None
    }
}

fn expect_i64(attr: &Attribute) -> Option<i64> {
    if let Expression::Number(n) = &attr.value {
        n.as_f64().map(|f| f as i64)
    } else {
        None
    }
}

fn expect_f64(attr: &Attribute) -> Option<f64> {
    if let Expression::Number(n) = &attr.value {
        n.as_f64()
    } else {
        None
    }
}

fn expect_bool(attr: &Attribute) -> Option<bool> {
    if let Expression::Bool(b) = &attr.value {
        Some(*b.as_ref())
    } else {
        None
    }
}

fn expect_string_list(value: &Expression) -> Vec<String> {
    let Expression::Array(arr) = value else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|item| {
            if let Expression::String(s) = item {
                Some(s.as_str().to_string())
            } else {
                None
            }
        })
        .collect()
}

fn extract_resource_ref(value: &Expression) -> Option<String> {
    let Expression::Traversal(t) = value else {
        return None;
    };
    let path = extract_traversal_path(t)?;
    if path.len() < 2 {
        return None;
    }
    Some(format!("{}.{}", path[0], path[1]))
}

fn extract_traversal_path(t: &Traversal) -> Option<Vec<String>> {
    let mut path = Vec::new();
    match &t.expr {
        Expression::Variable(v) => path.push(v.as_str().to_string()),
        _ => return None,
    }
    for op in t.operators.iter() {
        match &**op {
            TraversalOperator::GetAttr(name) => path.push(name.as_str().to_string()),
            _ => return None,
        }
    }
    Some(path)
}

fn missing(file: &ParsedFile, block: &Block, address: &str, field: &str) -> Diag {
    Diag::new(
        file.src.clone(),
        span_of(block.ident.span()),
        format!("{address} is missing required attribute '{field}'"),
    )
}

fn span_of(s: Option<std::ops::Range<usize>>) -> std::ops::Range<usize> {
    s.unwrap_or(0..0)
}

