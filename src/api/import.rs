use hcl_edit::Span;
use hcl_edit::expr::{Expression, Traversal, TraversalOperator};
use hcl_edit::structure::{Attribute, Block, Structure};

use crate::commands::export::{
    ExportInput, JsonAd, JsonAdGroup, JsonAdGroupAd, JsonAdGroupCriterion, JsonBudget,
    JsonCallAsset, JsonCampaign, JsonCampaignCriterion, JsonCampaignSharedSet,
    JsonConversionAction, JsonCustomerAsset, JsonKeyword, JsonLanguage, JsonLocation,
    JsonManualCpc, JsonNetworkSettings, JsonProximity, JsonResponsiveSearchAd, JsonRsaAsset,
    JsonSharedSet, JsonValueSettings,
};
use crate::diagnostics::Diag;
use crate::parser::ParsedFile;
use crate::schema::{ResourceRegistry, Resolution};

pub struct ImportResult {
    pub input: ExportInput,
    pub skipped: Vec<(String, String)>,
}

struct Ctx<'a> {
    file: &'a ParsedFile,
    registry: &'a ResourceRegistry,
}

impl<'a> Ctx<'a> {
    fn resolve_ref(&self, bare: &str) -> String {
        let mut parts = bare.splitn(2, '.');
        let Some(ty) = parts.next() else {
            return bare.to_string();
        };
        let Some(name) = parts.next() else {
            return bare.to_string();
        };
        match self.registry.resolve(&self.file.module, ty, name) {
            Resolution::Found(q) => q,
            _ => bare.to_string(),
        }
    }
}

pub fn import_files(files: &[ParsedFile]) -> Result<ImportResult, Vec<Diag>> {
    let (registry, mut diags) = ResourceRegistry::build(files);
    let mut input = ExportInput {
        customer_id: String::new(),
        login_customer_id: None,
        campaign_budgets: Vec::new(),
        campaigns: Vec::new(),
        ad_groups: Vec::new(),
        ad_group_ads: Vec::new(),
        ad_group_criteria: Vec::new(),
        campaign_criteria: Vec::new(),
        conversion_actions: Vec::new(),
        call_assets: Vec::new(),
        customer_assets: Vec::new(),
        shared_sets: Vec::new(),
        campaign_shared_sets: Vec::new(),
    };
    let mut skipped: Vec<(String, String)> = Vec::new();

    for f in files {
        let ctx = Ctx {
            file: f,
            registry: &registry,
        };
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
                    let address = ResourceRegistry::qualified(&f.module, ty, name);
                    let mut emit = |result: Result<(), Diag>| {
                        if let Err(d) = result {
                            diags.push(d);
                        }
                    };
                    match ty {
                        "google_ads_campaign_budget" => emit(
                            import_budget(&ctx, b, &address).map(|x| input.campaign_budgets.push(x)),
                        ),
                        "google_ads_campaign" => emit(
                            import_campaign(&ctx, b, &address).map(|x| input.campaigns.push(x)),
                        ),
                        "google_ads_ad_group" => emit(
                            import_ad_group(&ctx, b, &address).map(|x| input.ad_groups.push(x)),
                        ),
                        "google_ads_ad_group_ad" => emit(
                            import_ad_group_ad(&ctx, b, &address).map(|x| input.ad_group_ads.push(x)),
                        ),
                        "google_ads_ad_group_criterion" => emit(
                            import_ad_group_criterion(&ctx, b, &address).map(|xs| {
                                for x in xs {
                                    input.ad_group_criteria.push(x);
                                }
                            }),
                        ),
                        "google_ads_campaign_criterion" => emit(
                            import_campaign_criterion(&ctx, b, &address).map(|xs| {
                                for x in xs {
                                    input.campaign_criteria.push(x);
                                }
                            }),
                        ),
                        "google_ads_conversion_action" => emit(
                            import_conversion_action(&ctx, b, &address)
                                .map(|x| input.conversion_actions.push(x)),
                        ),
                        "google_ads_call_asset" => emit(
                            import_call_asset(&ctx, b, &address)
                                .map(|x| input.call_assets.push(x)),
                        ),
                        "google_ads_customer_asset" => emit(
                            import_customer_asset(&ctx, b, &address)
                                .map(|x| input.customer_assets.push(x)),
                        ),
                        "google_ads_shared_set" => emit(
                            import_shared_set(&ctx, b, &address)
                                .map(|x| input.shared_sets.push(x)),
                        ),
                        "google_ads_campaign_shared_set" => emit(
                            import_campaign_shared_set(&ctx, b, &address)
                                .map(|x| input.campaign_shared_sets.push(x)),
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

fn import_budget(ctx: &Ctx, block: &Block, address: &str) -> Result<JsonBudget, Diag> {
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

    let name = name.ok_or_else(|| missing(ctx.file, block, address, "name"))?;
    let amount = amount.ok_or_else(|| missing(ctx.file, block, address, "amount_micros"))?;
    Ok(JsonBudget {
        id: address.to_string(),
        name,
        amount_micros: amount,
        delivery_method,
        explicitly_shared,
    })
}

fn import_campaign(ctx: &Ctx, block: &Block, address: &str) -> Result<JsonCampaign, Diag> {
    let mut name = None;
    let mut status = None;
    let mut channel = None;
    let mut budget_ref = None;
    let mut eu_political = None;
    let mut manual_cpc = None;
    let mut network_settings = None;

    for s in block.body.iter() {
        match s {
            Structure::Attribute(a) => match a.key.as_str() {
                "name" => name = expect_string_owned(a),
                "status" => status = expect_string_owned(a),
                "advertising_channel_type" => channel = expect_string_owned(a),
                "campaign_budget" => budget_ref = extract_resource_ref(&a.value).map(|r| ctx.resolve_ref(&r)),
                "contains_eu_political_advertising" => eu_political = expect_string_owned(a),
                _ => {}
            },
            Structure::Block(b) => match b.ident.as_str() {
                "manual_cpc" => manual_cpc = Some(import_manual_cpc(b)),
                "network_settings" => network_settings = Some(import_network_settings(b)),
                _ => {}
            },
        }
    }

    let name = name.ok_or_else(|| missing(ctx.file, block, address, "name"))?;
    let channel = channel.ok_or_else(|| missing(ctx.file, block, address, "advertising_channel_type"))?;
    let budget = budget_ref.ok_or_else(|| missing(ctx.file, block, address, "campaign_budget"))?;

    Ok(JsonCampaign {
        id: address.to_string(),
        name,
        status,
        advertising_channel_type: channel,
        campaign_budget: budget,
        contains_eu_political_advertising: eu_political,
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

fn import_ad_group(ctx: &Ctx, block: &Block, address: &str) -> Result<JsonAdGroup, Diag> {
    let mut name = None;
    let mut campaign_ref = None;
    let mut status = None;
    let mut ty = None;
    let mut cpc = None;

    for s in block.body.iter() {
        let Structure::Attribute(a) = s else { continue };
        match a.key.as_str() {
            "name" => name = expect_string_owned(a),
            "campaign" => campaign_ref = extract_resource_ref(&a.value).map(|r| ctx.resolve_ref(&r)),
            "status" => status = expect_string_owned(a),
            "type" => ty = expect_string_owned(a),
            "cpc_bid_micros" => cpc = expect_i64(a),
            _ => {}
        }
    }

    let name = name.ok_or_else(|| missing(ctx.file, block, address, "name"))?;
    let campaign = campaign_ref.ok_or_else(|| missing(ctx.file, block, address, "campaign"))?;
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
    ctx: &Ctx,
    block: &Block,
    address: &str,
) -> Result<JsonAdGroupAd, Diag> {
    let mut ad_group_ref = None;
    let mut status = None;
    let mut ad = None;

    for s in block.body.iter() {
        match s {
            Structure::Attribute(a) => match a.key.as_str() {
                "ad_group" => ad_group_ref = extract_resource_ref(&a.value).map(|r| ctx.resolve_ref(&r)),
                "status" => status = expect_string_owned(a),
                _ => {}
            },
            Structure::Block(b) if b.ident.as_str() == "ad" => {
                ad = Some(import_ad(b));
            }
            _ => {}
        }
    }

    let ad_group = ad_group_ref.ok_or_else(|| missing(ctx.file, block, address, "ad_group"))?;
    let ad = ad.ok_or_else(|| missing(ctx.file, block, address, "ad"))?;
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
                "headlines" => headlines.extend(import_rsa_asset_list(&a.value)),
                "descriptions" => descriptions.extend(import_rsa_asset_list(&a.value)),
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

fn import_rsa_asset_list(value: &Expression) -> Vec<JsonRsaAsset> {
    let Expression::Array(arr) = value else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|item| match item {
            Expression::String(s) => Some(JsonRsaAsset {
                text: s.as_str().to_string(),
                pin: None,
            }),
            Expression::Object(obj) => {
                let mut text = None;
                let mut pin = None;
                for (key, val) in obj.iter() {
                    let Some(ident) = key.as_ident() else { continue };
                    match (ident.as_str(), val.expr()) {
                        ("text", Expression::String(s)) => {
                            text = Some(s.as_str().to_string());
                        }
                        ("pin", Expression::String(s)) => {
                            pin = Some(s.as_str().to_string());
                        }
                        _ => {}
                    }
                }
                Some(JsonRsaAsset { text: text?, pin })
            }
            _ => None,
        })
        .collect()
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
    ctx: &Ctx,
    block: &Block,
    address: &str,
) -> Result<Vec<JsonAdGroupCriterion>, Diag> {
    let mut ad_group_ref = None;
    let mut status = None;
    let mut negative = None;
    let mut cpc = None;
    let mut keywords: Vec<JsonKeyword> = Vec::new();
    let mut negative_keywords: Vec<JsonKeyword> = Vec::new();

    for s in block.body.iter() {
        match s {
            Structure::Attribute(a) => match a.key.as_str() {
                "ad_group" => ad_group_ref = extract_resource_ref(&a.value).map(|r| ctx.resolve_ref(&r)),
                "status" => status = expect_string_owned(a),
                "negative" => negative = expect_bool(a),
                "cpc_bid_micros" => cpc = expect_i64(a),
                _ => {}
            },
            Structure::Block(b) => match b.ident.as_str() {
                "keyword" => {
                    if let Some(kw) = import_keyword(b) {
                        keywords.push(kw);
                    }
                }
                "negative_keyword" => {
                    if let Some(kw) = import_keyword(b) {
                        negative_keywords.push(kw);
                    }
                }
                _ => {}
            },
        }
    }

    let ad_group = ad_group_ref.ok_or_else(|| missing(ctx.file, block, address, "ad_group"))?;

    let bulk = negative_keywords.len() + keywords.len() > 1 || !negative_keywords.is_empty();

    if bulk {
        if negative == Some(true) && !negative_keywords.is_empty() {
            return Err(Diag::new(
                ctx.file.src.clone(),
                span_of(block.ident.span()),
                format!(
                    "{address} sets negative = true alongside negative_keyword blocks; drop the attribute (the blocks are already negative)"
                ),
            ));
        }
        let mut out: Vec<JsonAdGroupCriterion> = Vec::new();
        for (i, kw) in keywords.into_iter().enumerate() {
            out.push(JsonAdGroupCriterion {
                id: format!("{address}.keywords[{i}]"),
                ad_group: ad_group.clone(),
                status: status.clone(),
                negative: negative.or(Some(false)),
                cpc_bid_micros: cpc,
                keyword: kw,
            });
        }
        for (i, kw) in negative_keywords.into_iter().enumerate() {
            out.push(JsonAdGroupCriterion {
                id: format!("{address}.negatives[{i}]"),
                ad_group: ad_group.clone(),
                status: status.clone(),
                negative: Some(true),
                cpc_bid_micros: None,
                keyword: kw,
            });
        }
        if out.is_empty() {
            return Err(missing(ctx.file, block, address, "keyword"));
        }
        return Ok(out);
    }

    let keyword = keywords
        .into_iter()
        .next()
        .ok_or_else(|| missing(ctx.file, block, address, "keyword"))?;
    Ok(vec![JsonAdGroupCriterion {
        id: address.to_string(),
        ad_group,
        status,
        negative: negative.or(Some(false)),
        cpc_bid_micros: cpc,
        keyword,
    }])
}

fn import_campaign_criterion(
    ctx: &Ctx,
    block: &Block,
    address: &str,
) -> Result<Vec<JsonCampaignCriterion>, Diag> {
    let mut campaign_ref = None;
    let mut status = None;
    let mut negative = None;
    let mut keyword = None;
    let mut location = None;
    let mut language = None;
    let mut proximity = None;
    let mut bulk_negatives: Vec<JsonKeyword> = Vec::new();

    for s in block.body.iter() {
        match s {
            Structure::Attribute(a) => match a.key.as_str() {
                "campaign" => campaign_ref = extract_resource_ref(&a.value).map(|r| ctx.resolve_ref(&r)),
                "status" => status = expect_string_owned(a),
                "negative" => negative = expect_bool(a),
                _ => {}
            },
            Structure::Block(b) => match b.ident.as_str() {
                "keyword" => keyword = import_keyword(b),
                "negative_keyword" => {
                    if let Some(kw) = import_keyword(b) {
                        bulk_negatives.push(kw);
                    }
                }
                "location" => location = import_location(b),
                "language" => language = import_language(b),
                "proximity" => proximity = import_proximity(b),
                _ => {}
            },
        }
    }

    let campaign = campaign_ref.ok_or_else(|| missing(ctx.file, block, address, "campaign"))?;

    if !bulk_negatives.is_empty() {
        if keyword.is_some() || location.is_some() || language.is_some() || proximity.is_some() {
            return Err(Diag::new(
                ctx.file.src.clone(),
                span_of(block.ident.span()),
                format!(
                    "{address} mixes negative_keyword blocks with a single-criterion form; pick one (a container resource is negatives-only)"
                ),
            ));
        }
        let mut out = Vec::with_capacity(bulk_negatives.len());
        for (i, kw) in bulk_negatives.into_iter().enumerate() {
            out.push(JsonCampaignCriterion {
                id: format!("{address}.negatives[{i}]"),
                campaign: campaign.clone(),
                status: status.clone(),
                negative: Some(true),
                keyword: Some(kw),
                location: None,
                language: None,
                proximity: None,
            });
        }
        return Ok(out);
    }

    let has_positive_shape =
        keyword.is_some() || location.is_some() || language.is_some() || proximity.is_some();
    Ok(vec![JsonCampaignCriterion {
        id: address.to_string(),
        campaign,
        status,
        negative: if has_positive_shape { negative.or(Some(false)) } else { negative },
        keyword,
        location,
        language,
        proximity,
    }])
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

fn import_conversion_action(
    ctx: &Ctx,
    block: &Block,
    address: &str,
) -> Result<JsonConversionAction, Diag> {
    let mut name = None;
    let mut ty = None;
    let mut category = None;
    let mut status = None;
    let mut counting_type = None;
    let mut click_lookback = None;
    let mut view_lookback = None;
    let mut value_settings = None;

    for s in block.body.iter() {
        match s {
            Structure::Attribute(a) => match a.key.as_str() {
                "name" => name = expect_string_owned(a),
                "type" => ty = expect_string_owned(a),
                "category" => category = expect_string_owned(a),
                "status" => status = expect_string_owned(a),
                "counting_type" => counting_type = expect_string_owned(a),
                "click_through_lookback_window_days" => click_lookback = expect_i64(a),
                "view_through_lookback_window_days" => view_lookback = expect_i64(a),
                _ => {}
            },
            Structure::Block(b) if b.ident.as_str() == "value_settings" => {
                let mut vs = JsonValueSettings {
                    default_value: None,
                    default_currency_code: None,
                    always_use_default_value: None,
                };
                for st in b.body.iter() {
                    if let Structure::Attribute(a) = st {
                        match a.key.as_str() {
                            "default_value" => vs.default_value = expect_f64(a),
                            "default_currency_code" => {
                                vs.default_currency_code = expect_string_owned(a)
                            }
                            "always_use_default_value" => {
                                vs.always_use_default_value = expect_bool(a)
                            }
                            _ => {}
                        }
                    }
                }
                value_settings = Some(vs);
            }
            _ => {}
        }
    }

    let name = name.ok_or_else(|| missing(ctx.file, block, address, "name"))?;
    let ty = ty.ok_or_else(|| missing(ctx.file, block, address, "type"))?;
    let category = category.ok_or_else(|| missing(ctx.file, block, address, "category"))?;
    Ok(JsonConversionAction {
        id: address.to_string(),
        name,
        ty,
        category,
        status,
        counting_type,
        click_through_lookback_window_days: click_lookback,
        view_through_lookback_window_days: view_lookback,
        value_settings,
    })
}

fn import_call_asset(
    ctx: &Ctx,
    block: &Block,
    address: &str,
) -> Result<JsonCallAsset, Diag> {
    let mut country_code = None;
    let mut phone_number = None;
    let mut reporting_state = None;
    let mut action_ref = None;

    for s in block.body.iter() {
        if let Structure::Attribute(a) = s {
            match a.key.as_str() {
                "country_code" => country_code = expect_string_owned(a),
                "phone_number" => phone_number = expect_string_owned(a),
                "call_conversion_reporting_state" => reporting_state = expect_string_owned(a),
                "call_conversion_action" => action_ref = extract_resource_ref(&a.value).map(|r| ctx.resolve_ref(&r)),
                _ => {}
            }
        }
    }
    let country_code = country_code.ok_or_else(|| missing(ctx.file, block, address, "country_code"))?;
    let phone_number = phone_number.ok_or_else(|| missing(ctx.file, block, address, "phone_number"))?;
    Ok(JsonCallAsset {
        id: address.to_string(),
        country_code,
        phone_number,
        call_conversion_reporting_state: reporting_state,
        call_conversion_action: action_ref,
    })
}

fn import_customer_asset(
    ctx: &Ctx,
    block: &Block,
    address: &str,
) -> Result<JsonCustomerAsset, Diag> {
    let mut asset_ref = None;
    let mut field_type = None;
    let mut status = None;
    for s in block.body.iter() {
        if let Structure::Attribute(a) = s {
            match a.key.as_str() {
                "asset" => asset_ref = extract_resource_ref(&a.value).map(|r| ctx.resolve_ref(&r)),
                "field_type" => field_type = expect_string_owned(a),
                "status" => status = expect_string_owned(a),
                _ => {}
            }
        }
    }
    let asset = asset_ref.ok_or_else(|| missing(ctx.file, block, address, "asset"))?;
    let field_type = field_type.ok_or_else(|| missing(ctx.file, block, address, "field_type"))?;
    Ok(JsonCustomerAsset {
        id: address.to_string(),
        asset,
        field_type,
        status,
    })
}

fn import_shared_set(
    ctx: &Ctx,
    block: &Block,
    address: &str,
) -> Result<JsonSharedSet, Diag> {
    let mut name = None;
    let mut ty = None;
    let mut status = None;
    let mut negative_keywords: Vec<JsonKeyword> = Vec::new();
    for s in block.body.iter() {
        match s {
            Structure::Attribute(a) => match a.key.as_str() {
                "name" => name = expect_string_owned(a),
                "type" => ty = expect_string_owned(a),
                "status" => status = expect_string_owned(a),
                _ => {}
            },
            Structure::Block(b) if b.ident.as_str() == "negative_keyword" => {
                if let Some(kw) = import_keyword(b) {
                    negative_keywords.push(kw);
                }
            }
            _ => {}
        }
    }
    let name = name.ok_or_else(|| missing(ctx.file, block, address, "name"))?;
    Ok(JsonSharedSet {
        id: address.to_string(),
        name,
        ty,
        status,
        negative_keywords,
    })
}

fn import_campaign_shared_set(
    ctx: &Ctx,
    block: &Block,
    address: &str,
) -> Result<JsonCampaignSharedSet, Diag> {
    let mut campaign_ref = None;
    let mut shared_set_ref = None;
    let mut status = None;
    for s in block.body.iter() {
        let Structure::Attribute(a) = s else { continue };
        match a.key.as_str() {
            "campaign" => campaign_ref = extract_resource_ref(&a.value).map(|r| ctx.resolve_ref(&r)),
            "shared_set" => shared_set_ref = extract_resource_ref(&a.value).map(|r| ctx.resolve_ref(&r)),
            "status" => status = expect_string_owned(a),
            _ => {}
        }
    }
    let campaign = campaign_ref.ok_or_else(|| missing(ctx.file, block, address, "campaign"))?;
    let shared_set = shared_set_ref.ok_or_else(|| missing(ctx.file, block, address, "shared_set"))?;
    Ok(JsonCampaignSharedSet {
        id: address.to_string(),
        campaign,
        shared_set,
        status,
    })
}

fn import_proximity(block: &Block) -> Option<JsonProximity> {
    let mut radius = None;
    let mut units = None;
    let mut latitude = None;
    let mut longitude = None;
    for s in block.body.iter() {
        if let Structure::Attribute(a) = s {
            match a.key.as_str() {
                "latitude" => latitude = expect_f64(a),
                "longitude" => longitude = expect_f64(a),
                "radius" => radius = expect_f64(a),
                "radius_units" => units = expect_string_owned(a),
                _ => {}
            }
        }
    }
    Some(JsonProximity {
        latitude: latitude?,
        longitude: longitude?,
        radius: radius?,
        radius_units: units?,
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

