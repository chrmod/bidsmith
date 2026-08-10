use hcl_edit::Span;
use hcl_edit::expr::{Expression, Traversal, TraversalOperator};
use hcl_edit::structure::{Attribute, Block, Structure};

use crate::commands::export::{
    ExportInput, JsonAd, JsonAdGroup, JsonAdGroupAd, JsonAdGroupAsset, JsonAdGroupCriterion,
    JsonBudget, JsonCallAsset, JsonCalloutAsset, JsonCampaign, JsonCampaignAsset,
    JsonCampaignCriterion, JsonCampaignSharedSet, JsonConversionAction, JsonCustomerAsset,
    JsonAgeRange, JsonAudience, JsonCustomAudience, JsonCustomAudienceMember,
    JsonDemandGenVideoResponsiveAd, JsonDevice, JsonFrequencyCap, JsonGender, JsonKeyword,
    JsonLanguage, JsonLocation, JsonManualCpc, JsonNetworkSettings,
    JsonProximity, JsonResponsiveSearchAd, JsonRsaAsset, JsonSharedCriterion, JsonSharedSet,
    JsonSitelinkAsset, JsonStructuredSnippetAsset, JsonTopic, JsonUserInterest, JsonValueSettings,
    JsonVideoResponsiveAd, JsonYoutubeChannel, JsonYoutubeVideo, JsonYoutubeVideoAsset,
};
use crate::diagnostics::Diag;
use crate::parser::ParsedFile;
use crate::program::Program;
use crate::schema::{
    ad_template_ref_name, AdTemplateRegistry, Bindings, DefaultsRegistry, InputBindings,
    ResourceRegistry, Resolution,
};

pub struct ImportResult {
    pub input: ExportInput,
    pub skipped: Vec<(String, String)>,
}

struct Ctx<'a> {
    file: &'a ParsedFile,
    registry: &'a ResourceRegistry,
    bindings: &'a Bindings,
    templates: &'a AdTemplateRegistry,
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

    fn resolve_value<'b>(&'b self, expr: &'b Expression) -> std::borrow::Cow<'b, Expression> {
        self.bindings.resolve_value(&self.file.module, expr)
    }
}

pub fn import_files(files: &[ParsedFile], inputs: &InputBindings) -> Result<ImportResult, Vec<Diag>> {
    let (expanded, mut diags) = crate::expand::expand_resource_for_each(files, inputs);
    let files = &expanded[..];
    let (registry, registry_diags) = ResourceRegistry::build(files);
    diags.extend(registry_diags);
    let (bindings, binding_diags) = Bindings::build(files, inputs);
    diags.extend(binding_diags);
    let (templates, _template_diags) = AdTemplateRegistry::build(files);
    // Defaults-block shape errors are validate's job; import merges best-effort.
    let (defaults, _defaults_diags) = DefaultsRegistry::build(files);
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
        sitelink_assets: Vec::new(),
        callout_assets: Vec::new(),
        structured_snippet_assets: Vec::new(),
        customer_assets: Vec::new(),
        campaign_assets: Vec::new(),
        ad_group_assets: Vec::new(),
        shared_sets: Vec::new(),
        shared_criteria: Vec::new(),
        campaign_shared_sets: Vec::new(),
        youtube_video_assets: Vec::new(),
        custom_audiences: Vec::new(),
        labels: Default::default(),
        claim_labels: Default::default(),
        campaign_claims: Default::default(),
        ad_group_claims: Default::default(),
    };
    let mut skipped: Vec<(String, String)> = Vec::new();

    for f in files {
        let ctx = Ctx {
            file: f,
            registry: &registry,
            bindings: &bindings,
            templates: &templates,
        };
        for s in f.body.iter() {
            let Structure::Block(b) = s else { continue };
            match b.ident.as_str() {
                "provider" => import_provider(&ctx, b, &mut input, &mut diags),
                "resource" => {
                    if b.labels.len() != 2 {
                        continue;
                    }
                    let ty = b.labels[0].as_str();
                    let name = b.labels[1].as_str();
                    let address = ResourceRegistry::qualified(&f.module, ty, name);
                    let merged_block;
                    let b = match defaults.merge(ty, b) {
                        Some(m) => {
                            merged_block = m;
                            &merged_block
                        }
                        None => b,
                    };
                    let mut emit = |result: Result<(), Diag>| {
                        if let Err(d) = result {
                            diags.push(d);
                        }
                    };
                    match ty {
                        "google_ads_campaign_budget" => emit(
                            import_budget(&ctx, b, &address).map(|x| input.campaign_budgets.push(x)),
                        ),
                        "google_ads_campaign" => emit(import_campaign(&ctx, b, &address).map(
                            |(campaign, criteria)| {
                                input.campaigns.push(campaign);
                                input.campaign_criteria.extend(criteria);
                            },
                        )),
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
                        "google_ads_sitelink_asset" => emit(
                            import_sitelink_asset(&ctx, b, &address)
                                .map(|x| input.sitelink_assets.push(x)),
                        ),
                        "google_ads_callout_asset" => emit(
                            import_callout_asset(&ctx, b, &address)
                                .map(|x| input.callout_assets.push(x)),
                        ),
                        "google_ads_structured_snippet_asset" => emit(
                            import_structured_snippet_asset(&ctx, b, &address)
                                .map(|x| input.structured_snippet_assets.push(x)),
                        ),
                        "google_ads_customer_asset" => emit(
                            import_customer_asset(&ctx, b, &address)
                                .map(|x| input.customer_assets.push(x)),
                        ),
                        "google_ads_campaign_asset" => emit(
                            import_campaign_asset(&ctx, b, &address)
                                .map(|x| input.campaign_assets.push(x)),
                        ),
                        "google_ads_ad_group_asset" => emit(
                            import_ad_group_asset(&ctx, b, &address)
                                .map(|x| input.ad_group_assets.push(x)),
                        ),
                        "google_ads_youtube_video_asset" => emit(
                            import_youtube_video_asset(&ctx, b, &address)
                                .map(|x| input.youtube_video_assets.push(x)),
                        ),
                        "google_ads_shared_set" => emit(
                            import_shared_set(&ctx, b, &address).map(|mut x| {
                                for (i, kw) in x.negative_keywords.iter().enumerate() {
                                    input.shared_criteria.push(JsonSharedCriterion {
                                        id: format!("{address}~{i}"),
                                        shared_set: address.clone(),
                                        keyword: kw.clone(),
                                    });
                                }
                                x.negative_keywords.clear();
                                input.shared_sets.push(x);
                            }),
                        ),
                        "google_ads_custom_audience" => emit(
                            import_custom_audience(&ctx, b, &address)
                                .map(|x| input.custom_audiences.push(x)),
                        ),
                        "google_ads_shared_criterion" => emit(
                            import_shared_criterion(&ctx, b, &address)
                                .map(|x| input.shared_criteria.push(x)),
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

    let resolved = crate::api::creds::Resolved::load();
    let block_customer =
        (!input.customer_id.is_empty()).then(|| std::mem::take(&mut input.customer_id));
    input.customer_id = crate::api::creds::env_nonempty("GOOGLE_ADS_CUSTOMER_ID")
        .or_else(|| resolved.project.customer_id.clone())
        .or(block_customer)
        .or_else(|| resolved.stored.customer_id.clone())
        .unwrap_or_default();
    let block_login = input.login_customer_id.take().filter(|s| !s.is_empty());
    input.login_customer_id = crate::api::creds::env_nonempty("GOOGLE_ADS_LOGIN_CUSTOMER_ID")
        .or_else(|| resolved.project.login_customer_id.clone())
        .or(block_login)
        .or_else(|| resolved.stored.login_customer_id.clone());

    if !diags.is_empty() {
        return Err(diags);
    }
    Ok(ImportResult { input, skipped })
}

fn import_provider(
    ctx: &Ctx,
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
                if let Some(v) = expect_string(ctx, a, diags) {
                    input.customer_id = v;
                }
            }
            "login_customer_id" => {
                if let Some(v) = expect_string(ctx, a, diags) {
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
            "name" => name = expect_string_owned(ctx, a),
            "amount_micros" => amount = expect_i64(ctx, a),
            "delivery_method" => delivery_method = expect_string_owned(ctx, a),
            "explicitly_shared" => explicitly_shared = expect_bool(ctx, a),
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

fn import_campaign(
    ctx: &Ctx,
    block: &Block,
    address: &str,
) -> Result<(JsonCampaign, Vec<JsonCampaignCriterion>), Diag> {
    let mut name = None;
    let mut status = None;
    let mut channel = None;
    let mut budget_ref = None;
    let mut eu_political = None;
    let mut manual_cpc = None;
    let mut network_settings = None;
    let mut frequency_caps: Vec<JsonFrequencyCap> = Vec::new();
    let mut languages: Vec<String> = Vec::new();
    let mut locations: Vec<String> = Vec::new();

    for s in block.body.iter() {
        match s {
            Structure::Attribute(a) => match a.key.as_str() {
                "name" => name = expect_string_owned(ctx, a),
                "status" => status = expect_string_owned(ctx, a),
                "advertising_channel_type" => channel = expect_string_owned(ctx, a),
                "campaign_budget" => budget_ref = extract_resource_ref(ctx, &a.value).map(|r| ctx.resolve_ref(&r)),
                "contains_eu_political_advertising" => eu_political = expect_string_owned(ctx, a),
                "languages" => languages = expect_string_list(ctx, &a.value),
                "locations" => locations = expect_string_list(ctx, &a.value),
                _ => {}
            },
            Structure::Block(b) => match b.ident.as_str() {
                "manual_cpc" => manual_cpc = Some(import_manual_cpc(ctx, b)),
                "network_settings" => network_settings = Some(import_network_settings(ctx, b)),
                "frequency_caps" => frequency_caps.extend(import_frequency_cap(ctx, b)),
                _ => {}
            },
        }
    }

    let name = name.ok_or_else(|| missing(ctx.file, block, address, "name"))?;
    let channel = channel.ok_or_else(|| missing(ctx.file, block, address, "advertising_channel_type"))?;
    let budget = budget_ref.ok_or_else(|| missing(ctx.file, block, address, "campaign_budget"))?;

    let criteria = expand_inline_targeting(address, &languages, &locations);

    Ok((
        JsonCampaign {
            id: address.to_string(),
            name,
            status,
            advertising_channel_type: channel,
            campaign_budget: budget,
            contains_eu_political_advertising: eu_political,
            manual_cpc,
            network_settings,
            frequency_caps,
            managed_address: None,
        },
        criteria,
    ))
}

/// Expand a campaign's inline `languages` / `locations` into one positive
/// campaign criterion each. Matched by criterion value (constant) at diff time,
/// so converting explicit criteria to inline — or adopting criteria already
/// live — is drift-free. Codes that don't resolve are skipped here (they were
/// already flagged by `validate`).
fn expand_inline_targeting(
    campaign_address: &str,
    languages: &[String],
    locations: &[String],
) -> Vec<JsonCampaignCriterion> {
    let (module, cname) = split_campaign_address(campaign_address);
    let mut out = Vec::new();
    let mut seen_lang: std::collections::HashSet<String> = std::collections::HashSet::new();
    for entry in languages {
        let Some(constant) = crate::targeting::resolve_language(entry) else {
            continue;
        };
        if !seen_lang.insert(constant.clone()) {
            continue;
        }
        let slug = crate::targeting::language_code(&constant)
            .map(str::to_string)
            .unwrap_or_else(|| last_path_segment(&constant).to_string());
        out.push(JsonCampaignCriterion {
            id: criterion_address(module, cname, "language", &slug),
            campaign: campaign_address.to_string(),
            status: Some("ENABLED".to_string()),
            negative: Some(false),
            bid_modifier: None,
            keyword: None,
            location: None,
            language: Some(JsonLanguage { language_constant: constant }),
            proximity: None,
            device: None,
            youtube_channel: None,
            youtube_video: None,
            topic: None,
            user_interest: None,
            age_range: None,
            gender: None,
            audience: None,
        });
    }
    let mut seen_loc: std::collections::HashSet<String> = std::collections::HashSet::new();
    for entry in locations {
        let Some(constant) = crate::targeting::resolve_location(entry) else {
            continue;
        };
        if !seen_loc.insert(constant.clone()) {
            continue;
        }
        let slug = crate::targeting::location_code(&constant)
            .map(str::to_string)
            .unwrap_or_else(|| last_path_segment(&constant).to_string());
        out.push(JsonCampaignCriterion {
            id: criterion_address(module, cname, "location", &slug),
            campaign: campaign_address.to_string(),
            status: Some("ENABLED".to_string()),
            negative: Some(false),
            bid_modifier: None,
            keyword: None,
            location: Some(JsonLocation { geo_target_constant: constant }),
            language: None,
            proximity: None,
            device: None,
            youtube_channel: None,
            youtube_video: None,
            topic: None,
            user_interest: None,
            age_range: None,
            gender: None,
            audience: None,
        });
    }
    out
}

// Campaign address is `<module>.google_ads_campaign.<name>`; module may itself
// contain dots (a for_each instance), but the type and name segments never do.
fn split_campaign_address(address: &str) -> (&str, &str) {
    let (module_and_type, name) = address.rsplit_once('.').unwrap_or(("", address));
    let (module, _ty) = module_and_type.rsplit_once('.').unwrap_or(("", module_and_type));
    (module, name)
}

fn criterion_address(module: &str, campaign_name: &str, axis: &str, slug: &str) -> String {
    let local = format!("google_ads_campaign_criterion.{campaign_name}_{axis}_{}", slugify_id(slug));
    if module.is_empty() {
        local
    } else {
        format!("{module}.{local}")
    }
}

fn slugify_id(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push('_');
        }
    }
    out
}

fn last_path_segment(s: &str) -> &str {
    s.rsplit('/').next().unwrap_or(s)
}

fn import_manual_cpc(ctx: &Ctx, block: &Block) -> JsonManualCpc {
    let mut enhanced = None;
    for s in block.body.iter() {
        if let Structure::Attribute(a) = s {
            if a.key.as_str() == "enhanced_cpc_enabled" {
                enhanced = expect_bool(ctx, a);
            }
        }
    }
    JsonManualCpc {
        enhanced_cpc_enabled: enhanced,
    }
}

fn import_network_settings(ctx: &Ctx, block: &Block) -> JsonNetworkSettings {
    let mut s = JsonNetworkSettings {
        target_google_search: None,
        target_search_network: None,
        target_content_network: None,
        target_partner_search_network: None,
    };
    for st in block.body.iter() {
        if let Structure::Attribute(a) = st {
            match a.key.as_str() {
                "target_google_search" => s.target_google_search = expect_bool(ctx, a),
                "target_search_network" => s.target_search_network = expect_bool(ctx, a),
                "target_content_network" => s.target_content_network = expect_bool(ctx, a),
                "target_partner_search_network" => s.target_partner_search_network = expect_bool(ctx, a),
                _ => {}
            }
        }
    }
    s
}

/// `None` when a required attribute is missing or non-literal — `validate`
/// already reported it, and a half-specified cap must not reach the API.
fn import_frequency_cap(ctx: &Ctx, block: &Block) -> Option<JsonFrequencyCap> {
    let mut event_type = None;
    let mut time_unit = None;
    let mut time_length = None;
    let mut cap = None;
    let mut level = None;
    for st in block.body.iter() {
        if let Structure::Attribute(a) = st {
            match a.key.as_str() {
                "event_type" => event_type = expect_string_owned(ctx, a),
                "time_unit" => time_unit = expect_string_owned(ctx, a),
                "time_length" => time_length = expect_i64(ctx, a),
                "cap" => cap = expect_i64(ctx, a),
                "level" => level = expect_string_owned(ctx, a),
                _ => {}
            }
        }
    }
    Some(JsonFrequencyCap {
        event_type: event_type?,
        time_unit: time_unit?,
        time_length: time_length?,
        cap: cap?,
        level,
    })
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
            "name" => name = expect_string_owned(ctx, a),
            "campaign" => campaign_ref = extract_resource_ref(ctx, &a.value).map(|r| ctx.resolve_ref(&r)),
            "status" => status = expect_string_owned(ctx, a),
            "type" => ty = expect_string_owned(ctx, a),
            "cpc_bid_micros" => cpc = expect_i64(ctx, a),
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
        managed_address: None,
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
    let mut template: Option<&Attribute> = None;
    let mut final_urls_override: Option<Vec<String>> = None;
    let mut path1_override: Option<String> = None;
    let mut path2_override: Option<String> = None;

    for s in block.body.iter() {
        match s {
            Structure::Attribute(a) => match a.key.as_str() {
                "ad_group" => ad_group_ref = extract_resource_ref(ctx, &a.value).map(|r| ctx.resolve_ref(&r)),
                "status" => status = expect_string_owned(ctx, a),
                "template" => template = Some(a),
                "final_urls" => final_urls_override = Some(expect_string_list(ctx, &a.value)),
                "path1" => path1_override = expect_string_owned(ctx, a),
                "path2" => path2_override = expect_string_owned(ctx, a),
                _ => {}
            },
            Structure::Block(b) if b.ident.as_str() == "ad" => {
                ad = Some(import_ad(ctx, b));
            }
            _ => {}
        }
    }

    // Expand `template = ad_template.<name>` into the template's body — same mutate as an inline `ad {}`, own address kept.
    // Per-instance overrides on the resource (final_urls, RSA path1/path2) take precedence over the template body.
    if ad.is_none() {
        if let Some(a) = template {
            let mut resolved = resolve_ad_template(ctx, a)?;
            apply_ad_overrides(&mut resolved, final_urls_override, path1_override, path2_override);
            ad = Some(resolved);
        }
    }

    let ad_group = ad_group_ref.ok_or_else(|| missing(ctx.file, block, address, "ad_group"))?;
    let ad = ad.ok_or_else(|| missing(ctx.file, block, address, "ad"))?;
    Ok(JsonAdGroupAd {
        id: address.to_string(),
        ad_group,
        status,
        ad,
        managed_address: None,
    })
}

fn resolve_ad_template(ctx: &Ctx, attr: &Attribute) -> Result<JsonAd, Diag> {
    let invalid = || {
        Diag::new(
            ctx.file.src.clone(),
            span_of(attr.value.span()),
            "template must be a reference of the form ad_template.<name>".to_string(),
        )
    };
    let name = ad_template_ref_name(&attr.value).ok_or_else(invalid)?;
    match ctx.templates.resolve(&ctx.file.module, &name) {
        Resolution::Found(q) => match ctx.templates.get(&q) {
            Some(decl) => Ok(import_ad(ctx, &decl.block)),
            None => Err(invalid()),
        },
        _ => Err(Diag::new(
            ctx.file.src.clone(),
            span_of(attr.value.span()),
            format!("reference to undeclared ad_template 'ad_template.{name}'"),
        )),
    }
}

fn apply_ad_overrides(
    ad: &mut JsonAd,
    final_urls: Option<Vec<String>>,
    path1: Option<String>,
    path2: Option<String>,
) {
    if let Some(urls) = final_urls {
        if !urls.is_empty() {
            ad.final_urls = urls;
        }
    }
    if let Some(rsa) = ad.responsive_search_ad.as_mut() {
        if path1.is_some() {
            rsa.path1 = path1;
        }
        if path2.is_some() {
            rsa.path2 = path2;
        }
    }
}

fn import_ad(ctx: &Ctx, block: &Block) -> JsonAd {
    let mut name = None;
    let mut final_urls: Vec<String> = Vec::new();
    let mut rsa = None;
    let mut video = None;
    let mut demand_gen = None;

    for s in block.body.iter() {
        match s {
            Structure::Attribute(a) => match a.key.as_str() {
                "name" => name = expect_string_owned(ctx, a),
                "final_urls" => final_urls = expect_string_list(ctx, &a.value),
                _ => {}
            },
            Structure::Block(b) => match b.ident.as_str() {
                "responsive_search_ad" => rsa = Some(import_rsa(ctx, b)),
                "video_responsive_ad" => video = Some(import_video_ad(ctx, b)),
                "demand_gen_video_responsive_ad" => {
                    demand_gen = Some(import_demand_gen_video_ad(ctx, b))
                }
                _ => {}
            },
        }
    }

    JsonAd {
        name,
        final_urls,
        responsive_search_ad: rsa,
        video_responsive_ad: video,
        demand_gen_video_responsive_ad: demand_gen,
    }
}

fn import_demand_gen_video_ad(ctx: &Ctx, block: &Block) -> JsonDemandGenVideoResponsiveAd {
    let mut videos: Vec<String> = Vec::new();
    let mut headlines: Vec<String> = Vec::new();
    let mut long_headlines: Vec<String> = Vec::new();
    let mut descriptions: Vec<String> = Vec::new();
    let mut call_to_actions: Vec<String> = Vec::new();
    let mut breadcrumb1 = None;
    let mut breadcrumb2 = None;
    let mut business_name = None;
    for s in block.body.iter() {
        let Structure::Attribute(a) = s else { continue };
        match a.key.as_str() {
            "videos" => {
                videos = extract_resource_ref_list(ctx, &a.value)
                    .into_iter()
                    .map(|r| ctx.resolve_ref(&r))
                    .collect();
            }
            "headlines" => headlines = expect_string_list(ctx, &a.value),
            "long_headlines" => long_headlines = expect_string_list(ctx, &a.value),
            "descriptions" => descriptions = expect_string_list(ctx, &a.value),
            "call_to_actions" => call_to_actions = expect_string_list(ctx, &a.value),
            "breadcrumb1" => breadcrumb1 = expect_string_owned(ctx, a),
            "breadcrumb2" => breadcrumb2 = expect_string_owned(ctx, a),
            "business_name" => business_name = expect_string_owned(ctx, a),
            _ => {}
        }
    }
    JsonDemandGenVideoResponsiveAd {
        videos,
        headlines,
        long_headlines,
        descriptions,
        call_to_actions,
        breadcrumb1,
        breadcrumb2,
        business_name,
    }
}

fn import_video_ad(ctx: &Ctx, block: &Block) -> JsonVideoResponsiveAd {
    let mut video = None;
    let mut headlines: Vec<String> = Vec::new();
    let mut long_headlines: Vec<String> = Vec::new();
    let mut descriptions: Vec<String> = Vec::new();
    let mut call_to_actions: Vec<String> = Vec::new();
    for s in block.body.iter() {
        let Structure::Attribute(a) = s else { continue };
        match a.key.as_str() {
            "video" => {
                video = extract_resource_ref(ctx, &a.value).map(|r| ctx.resolve_ref(&r));
            }
            "headlines" => headlines = expect_string_list(ctx, &a.value),
            "long_headlines" => long_headlines = expect_string_list(ctx, &a.value),
            "descriptions" => descriptions = expect_string_list(ctx, &a.value),
            "call_to_actions" => call_to_actions = expect_string_list(ctx, &a.value),
            _ => {}
        }
    }
    JsonVideoResponsiveAd {
        video: video.unwrap_or_default(),
        headlines,
        long_headlines,
        descriptions,
        call_to_actions,
    }
}

fn import_youtube_video_asset(
    ctx: &Ctx,
    block: &Block,
    address: &str,
) -> Result<JsonYoutubeVideoAsset, Diag> {
    let mut youtube_video_id = None;
    let mut youtube_video_title = None;
    for s in block.body.iter() {
        if let Structure::Attribute(a) = s {
            match a.key.as_str() {
                "youtube_video_id" => youtube_video_id = expect_string_owned(ctx, a),
                "youtube_video_title" => youtube_video_title = expect_string_owned(ctx, a),
                _ => {}
            }
        }
    }
    let youtube_video_id =
        youtube_video_id.ok_or_else(|| missing(ctx.file, block, address, "youtube_video_id"))?;
    Ok(JsonYoutubeVideoAsset {
        id: address.to_string(),
        youtube_video_id,
        youtube_video_title,
    })
}

fn import_rsa(ctx: &Ctx, block: &Block) -> JsonResponsiveSearchAd {
    let mut path1 = None;
    let mut path2 = None;
    let mut headlines: Vec<JsonRsaAsset> = Vec::new();
    let mut descriptions: Vec<JsonRsaAsset> = Vec::new();

    for s in block.body.iter() {
        match s {
            Structure::Attribute(a) => match a.key.as_str() {
                "path1" => path1 = expect_string_owned(ctx, a),
                "path2" => path2 = expect_string_owned(ctx, a),
                "headlines" => headlines.extend(import_rsa_asset_list(ctx, &a.value)),
                "descriptions" => descriptions.extend(import_rsa_asset_list(ctx, &a.value)),
                _ => {}
            },
            Structure::Block(b) => match b.ident.as_str() {
                "headline" => {
                    if let Some(asset) = import_rsa_asset(ctx, b) {
                        headlines.push(asset);
                    }
                }
                "description" => {
                    if let Some(asset) = import_rsa_asset(ctx, b) {
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

fn import_rsa_asset_list(ctx: &Ctx, value: &Expression) -> Vec<JsonRsaAsset> {
    let resolved = ctx.resolve_value(value);
    let Expression::Array(arr) = resolved.as_ref() else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|item| match ctx.resolve_value(item).as_ref() {
            Expression::String(s) => Some(JsonRsaAsset {
                text: s.as_str().to_string(),
                pin: None,
            }),
            Expression::Object(obj) => {
                let mut text = None;
                let mut pin = None;
                for (key, val) in obj.iter() {
                    let Some(ident) = key.as_ident() else { continue };
                    match (ident.as_str(), ctx.resolve_value(val.expr()).as_ref()) {
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

fn import_rsa_asset(ctx: &Ctx, block: &Block) -> Option<JsonRsaAsset> {
    let mut text = None;
    let mut pin = None;
    for s in block.body.iter() {
        if let Structure::Attribute(a) = s {
            match a.key.as_str() {
                "text" => text = expect_string_owned(ctx, a),
                "pin" => pin = expect_string_owned(ctx, a),
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
                "ad_group" => ad_group_ref = extract_resource_ref(ctx, &a.value).map(|r| ctx.resolve_ref(&r)),
                "status" => status = expect_string_owned(ctx, a),
                "negative" => negative = expect_bool(ctx, a),
                "cpc_bid_micros" => cpc = expect_i64(ctx, a),
                _ => {}
            },
            Structure::Block(b) => match b.ident.as_str() {
                "keyword" => {
                    if let Some(kw) = import_keyword(ctx, b) {
                        keywords.push(kw);
                    }
                }
                "keywords" => keywords.extend(import_compact_keywords(ctx, b)),
                "negative_keyword" => {
                    if let Some(kw) = import_keyword(ctx, b) {
                        negative_keywords.push(kw);
                    }
                }
                "negative_keywords" => negative_keywords.extend(import_compact_keywords(ctx, b)),
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
                managed_address: None,
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
                managed_address: None,
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
        managed_address: None,
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
    let mut bid_modifier = None;
    let mut keyword = None;
    let mut location = None;
    let mut language = None;
    let mut proximity = None;
    let mut device = None;
    let mut youtube_channel = None;
    let mut youtube_video = None;
    let mut topic = None;
    let mut user_interest = None;
    let mut age_range = None;
    let mut gender = None;
    let mut audience = None;
    let mut bulk_negatives: Vec<JsonKeyword> = Vec::new();

    for s in block.body.iter() {
        match s {
            Structure::Attribute(a) => match a.key.as_str() {
                "campaign" => campaign_ref = extract_resource_ref(ctx, &a.value).map(|r| ctx.resolve_ref(&r)),
                "status" => status = expect_string_owned(ctx, a),
                "negative" => negative = expect_bool(ctx, a),
                "bid_modifier" => bid_modifier = expect_f64(ctx, a),
                _ => {}
            },
            Structure::Block(b) => match b.ident.as_str() {
                "keyword" => keyword = import_keyword(ctx, b),
                "negative_keyword" => {
                    if let Some(kw) = import_keyword(ctx, b) {
                        bulk_negatives.push(kw);
                    }
                }
                "negative_keywords" => bulk_negatives.extend(import_compact_keywords(ctx, b)),
                "location" => location = import_location(ctx, b),
                "language" => language = import_language(ctx, b),
                "proximity" => proximity = import_proximity(ctx, b),
                "device" => device = import_device(ctx, b),
                "youtube_channel" => {
                    youtube_channel = one_string(ctx, b, "channel_id")
                        .map(|channel_id| JsonYoutubeChannel { channel_id })
                }
                "youtube_video" => {
                    youtube_video =
                        one_string(ctx, b, "video_id").map(|video_id| JsonYoutubeVideo { video_id })
                }
                "topic" => {
                    topic = one_string(ctx, b, "topic_constant")
                        .map(|topic_constant| JsonTopic { topic_constant })
                }
                "user_interest" => {
                    user_interest = one_string(ctx, b, "user_interest_category")
                        .map(|user_interest_category| JsonUserInterest {
                            user_interest_category,
                        })
                }
                "age_range" => age_range = one_string(ctx, b, "type").map(|ty| JsonAgeRange { ty }),
                "gender" => gender = one_string(ctx, b, "type").map(|ty| JsonGender { ty }),
                "audience" => audience = import_audience(ctx, b),
                _ => {}
            },
        }
    }

    let campaign = campaign_ref.ok_or_else(|| missing(ctx.file, block, address, "campaign"))?;

    if !bulk_negatives.is_empty() {
        if keyword.is_some()
            || location.is_some()
            || language.is_some()
            || proximity.is_some()
            || device.is_some()
        {
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
                bid_modifier: None,
                keyword: Some(kw),
                location: None,
                language: None,
                proximity: None,
                device: None,
                youtube_channel: None,
                youtube_video: None,
                topic: None,
                user_interest: None,
                age_range: None,
                gender: None,
                audience: None,
            });
        }
        return Ok(out);
    }

    let has_positive_shape = keyword.is_some()
        || location.is_some()
        || language.is_some()
        || proximity.is_some()
        || device.is_some()
        || youtube_channel.is_some()
        || youtube_video.is_some()
        || topic.is_some()
        || user_interest.is_some()
        || age_range.is_some()
        || gender.is_some()
        || audience.is_some();
    Ok(vec![JsonCampaignCriterion {
        id: address.to_string(),
        campaign,
        status,
        negative: if has_positive_shape { negative.or(Some(false)) } else { negative },
        bid_modifier,
        keyword,
        location,
        language,
        proximity,
        device,
        youtube_channel,
        youtube_video,
        topic,
        user_interest,
        age_range,
        gender,
        audience,
    }])
}

/// A single-attribute criterion block: the value, or `None` when unset or
/// non-literal (`validate` already reported it).
fn one_string(ctx: &Ctx, block: &Block, key: &str) -> Option<String> {
    block.body.iter().find_map(|s| match s {
        Structure::Attribute(a) if a.key.as_str() == key => expect_string_owned(ctx, a),
        _ => None,
    })
}

fn import_audience(ctx: &Ctx, block: &Block) -> Option<JsonAudience> {
    let mut out = JsonAudience {
        custom_audience: None,
        user_list: None,
        combined_audience: None,
    };
    for s in block.body.iter() {
        let Structure::Attribute(a) = s else { continue };
        match a.key.as_str() {
            // A declared google_ads_custom_audience is referenced by address;
            // a segment built elsewhere is named by its API resource name.
            "custom_audience" => {
                out.custom_audience = extract_resource_ref(ctx, &a.value)
                    .map(|r| ctx.resolve_ref(&r))
                    .or_else(|| expect_string_owned(ctx, a));
            }
            "user_list" => out.user_list = expect_string_owned(ctx, a),
            "combined_audience" => out.combined_audience = expect_string_owned(ctx, a),
            _ => {}
        }
    }
    out.source().is_some().then_some(out)
}

fn import_custom_audience(
    ctx: &Ctx,
    block: &Block,
    address: &str,
) -> Result<JsonCustomAudience, Diag> {
    let mut name = None;
    let mut description = None;
    let mut ty = None;
    let mut status = None;
    let mut members: Vec<JsonCustomAudienceMember> = Vec::new();
    for s in block.body.iter() {
        match s {
            Structure::Attribute(a) => match a.key.as_str() {
                "name" => name = expect_string_owned(ctx, a),
                "description" => description = expect_string_owned(ctx, a),
                "type" => ty = expect_string_owned(ctx, a),
                "status" => status = expect_string_owned(ctx, a),
                _ => {}
            },
            Structure::Block(b) if b.ident.as_str() == "member" => {
                members.extend(import_custom_audience_member(ctx, b));
            }
            Structure::Block(_) => {}
        }
    }
    let name = name.ok_or_else(|| missing(ctx.file, block, address, "name"))?;
    Ok(JsonCustomAudience {
        id: address.to_string(),
        name,
        description,
        ty,
        status,
        members,
    })
}

fn import_custom_audience_member(ctx: &Ctx, block: &Block) -> Option<JsonCustomAudienceMember> {
    let mut m = JsonCustomAudienceMember {
        keyword: None,
        url: None,
        place_category: None,
        app: None,
    };
    for s in block.body.iter() {
        let Structure::Attribute(a) = s else { continue };
        match a.key.as_str() {
            "keyword" => m.keyword = expect_string_owned(ctx, a),
            "url" => m.url = expect_string_owned(ctx, a),
            "place_category" => m.place_category = expect_string_owned(ctx, a),
            "app" => m.app = expect_string_owned(ctx, a),
            _ => {}
        }
    }
    m.payload().is_some().then_some(m)
}

fn import_keyword(ctx: &Ctx, block: &Block) -> Option<JsonKeyword> {
    let mut text = None;
    let mut match_type = None;
    for s in block.body.iter() {
        if let Structure::Attribute(a) = s {
            match a.key.as_str() {
                "text" => text = expect_string_owned(ctx, a),
                "match_type" => match_type = expect_string_owned(ctx, a),
                _ => {}
            }
        }
    }
    Some(JsonKeyword {
        text: text?,
        match_type: match_type?,
    })
}

// Match-type-major order keeps each match type's criteria grouped together.
fn import_compact_keywords(ctx: &Ctx, block: &Block) -> Vec<JsonKeyword> {
    let mut texts: Vec<String> = Vec::new();
    let mut match_types: Vec<String> = Vec::new();
    for s in block.body.iter() {
        let Structure::Attribute(a) = s else { continue };
        match a.key.as_str() {
            "texts" => texts = expect_string_list(ctx, &a.value),
            "match_type" => {
                if let Some(mt) = expect_string_owned(ctx, a) {
                    match_types.push(mt);
                }
            }
            "match_types" => match_types.extend(expect_string_list(ctx, &a.value)),
            _ => {}
        }
    }
    let mut out = Vec::with_capacity(texts.len() * match_types.len());
    for match_type in &match_types {
        for text in &texts {
            out.push(JsonKeyword {
                text: text.clone(),
                match_type: match_type.clone(),
            });
        }
    }
    out
}

fn import_location(ctx: &Ctx, block: &Block) -> Option<JsonLocation> {
    let mut geo = None;
    for s in block.body.iter() {
        if let Structure::Attribute(a) = s {
            if a.key.as_str() == "geo_target_constant" {
                geo = expect_string_owned(ctx, a);
            }
        }
    }
    Some(JsonLocation {
        geo_target_constant: geo?,
    })
}

fn import_language(ctx: &Ctx, block: &Block) -> Option<JsonLanguage> {
    let mut lang = None;
    for s in block.body.iter() {
        if let Structure::Attribute(a) = s {
            if a.key.as_str() == "language_constant" {
                lang = expect_string_owned(ctx, a);
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
                "name" => name = expect_string_owned(ctx, a),
                "type" => ty = expect_string_owned(ctx, a),
                "category" => category = expect_string_owned(ctx, a),
                "status" => status = expect_string_owned(ctx, a),
                "counting_type" => counting_type = expect_string_owned(ctx, a),
                "click_through_lookback_window_days" => click_lookback = expect_i64(ctx, a),
                "view_through_lookback_window_days" => view_lookback = expect_i64(ctx, a),
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
                            "default_value" => vs.default_value = expect_f64(ctx, a),
                            "default_currency_code" => {
                                vs.default_currency_code = expect_string_owned(ctx, a)
                            }
                            "always_use_default_value" => {
                                vs.always_use_default_value = expect_bool(ctx, a)
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
                "country_code" => country_code = expect_string_owned(ctx, a),
                "phone_number" => phone_number = expect_string_owned(ctx, a),
                "call_conversion_reporting_state" => reporting_state = expect_string_owned(ctx, a),
                "call_conversion_action" => {
                    action_ref = extract_resource_ref(ctx, &a.value)
                        .map(|r| ctx.resolve_ref(&r))
                        .or_else(|| expect_string_owned(ctx, a));
                }
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
                "asset" => asset_ref = extract_resource_ref(ctx, &a.value).map(|r| ctx.resolve_ref(&r)),
                "field_type" => field_type = expect_string_owned(ctx, a),
                "status" => status = expect_string_owned(ctx, a),
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

fn import_sitelink_asset(
    ctx: &Ctx,
    block: &Block,
    address: &str,
) -> Result<JsonSitelinkAsset, Diag> {
    let mut link_text = None;
    let mut description1 = None;
    let mut description2 = None;
    let mut final_urls: Vec<String> = Vec::new();
    for s in block.body.iter() {
        if let Structure::Attribute(a) = s {
            match a.key.as_str() {
                "link_text" => link_text = expect_string_owned(ctx, a),
                "description1" => description1 = expect_string_owned(ctx, a),
                "description2" => description2 = expect_string_owned(ctx, a),
                "final_urls" => final_urls = expect_string_list(ctx, &a.value),
                _ => {}
            }
        }
    }
    let link_text = link_text.ok_or_else(|| missing(ctx.file, block, address, "link_text"))?;
    Ok(JsonSitelinkAsset {
        id: address.to_string(),
        link_text,
        description1,
        description2,
        final_urls,
    })
}

fn import_callout_asset(
    ctx: &Ctx,
    block: &Block,
    address: &str,
) -> Result<JsonCalloutAsset, Diag> {
    let mut text = None;
    for s in block.body.iter() {
        if let Structure::Attribute(a) = s {
            if a.key.as_str() == "text" {
                text = expect_string_owned(ctx, a);
            }
        }
    }
    let text = text.ok_or_else(|| missing(ctx.file, block, address, "text"))?;
    Ok(JsonCalloutAsset {
        id: address.to_string(),
        text,
    })
}

fn import_structured_snippet_asset(
    ctx: &Ctx,
    block: &Block,
    address: &str,
) -> Result<JsonStructuredSnippetAsset, Diag> {
    let mut header = None;
    let mut values: Vec<String> = Vec::new();
    for s in block.body.iter() {
        if let Structure::Attribute(a) = s {
            match a.key.as_str() {
                "header" => header = expect_string_owned(ctx, a),
                "values" => values = expect_string_list(ctx, &a.value),
                _ => {}
            }
        }
    }
    let header = header.ok_or_else(|| missing(ctx.file, block, address, "header"))?;
    Ok(JsonStructuredSnippetAsset {
        id: address.to_string(),
        header,
        values,
    })
}

fn import_campaign_asset(
    ctx: &Ctx,
    block: &Block,
    address: &str,
) -> Result<JsonCampaignAsset, Diag> {
    let mut campaign_ref = None;
    let mut asset_ref = None;
    let mut field_type = None;
    let mut status = None;
    for s in block.body.iter() {
        if let Structure::Attribute(a) = s {
            match a.key.as_str() {
                "campaign" => {
                    campaign_ref = extract_resource_ref(ctx, &a.value)
                        .map(|r| ctx.resolve_ref(&r))
                        .or_else(|| expect_string_owned(ctx, a));
                }
                "asset" => {
                    asset_ref = extract_resource_ref(ctx, &a.value).map(|r| ctx.resolve_ref(&r))
                }
                "field_type" => field_type = expect_string_owned(ctx, a),
                "status" => status = expect_string_owned(ctx, a),
                _ => {}
            }
        }
    }
    let campaign = campaign_ref.ok_or_else(|| missing(ctx.file, block, address, "campaign"))?;
    let asset = asset_ref.ok_or_else(|| missing(ctx.file, block, address, "asset"))?;
    let field_type = field_type.ok_or_else(|| missing(ctx.file, block, address, "field_type"))?;
    Ok(JsonCampaignAsset {
        id: address.to_string(),
        campaign,
        asset,
        field_type,
        status,
    })
}

fn import_ad_group_asset(
    ctx: &Ctx,
    block: &Block,
    address: &str,
) -> Result<JsonAdGroupAsset, Diag> {
    let mut ad_group_ref = None;
    let mut asset_ref = None;
    let mut field_type = None;
    let mut status = None;
    for s in block.body.iter() {
        if let Structure::Attribute(a) = s {
            match a.key.as_str() {
                "ad_group" => {
                    ad_group_ref = extract_resource_ref(ctx, &a.value)
                        .map(|r| ctx.resolve_ref(&r))
                        .or_else(|| expect_string_owned(ctx, a));
                }
                "asset" => {
                    asset_ref = extract_resource_ref(ctx, &a.value).map(|r| ctx.resolve_ref(&r))
                }
                "field_type" => field_type = expect_string_owned(ctx, a),
                "status" => status = expect_string_owned(ctx, a),
                _ => {}
            }
        }
    }
    let ad_group = ad_group_ref.ok_or_else(|| missing(ctx.file, block, address, "ad_group"))?;
    let asset = asset_ref.ok_or_else(|| missing(ctx.file, block, address, "asset"))?;
    let field_type = field_type.ok_or_else(|| missing(ctx.file, block, address, "field_type"))?;
    Ok(JsonAdGroupAsset {
        id: address.to_string(),
        ad_group,
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
                "name" => name = expect_string_owned(ctx, a),
                "type" => ty = expect_string_owned(ctx, a),
                "status" => status = expect_string_owned(ctx, a),
                _ => {}
            },
            Structure::Block(b) => match b.ident.as_str() {
                "negative_keyword" => {
                    if let Some(kw) = import_keyword(ctx, b) {
                        negative_keywords.push(kw);
                    }
                }
                "negative_keywords" => negative_keywords.extend(import_compact_keywords(ctx, b)),
                _ => {}
            },
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

fn import_shared_criterion(
    ctx: &Ctx,
    block: &Block,
    address: &str,
) -> Result<JsonSharedCriterion, Diag> {
    let mut shared_set_ref = None;
    let mut keyword: Option<JsonKeyword> = None;
    for s in block.body.iter() {
        match s {
            Structure::Attribute(a) => {
                if a.key.as_str() == "shared_set" {
                    shared_set_ref = extract_resource_ref(ctx, &a.value)
                        .map(|r| ctx.resolve_ref(&r))
                        .or_else(|| expect_string_owned(ctx, a));
                }
            }
            Structure::Block(b) if b.ident.as_str() == "keyword" => {
                if let Some(kw) = import_keyword(ctx, b) {
                    keyword = Some(kw);
                }
            }
            _ => {}
        }
    }
    let shared_set =
        shared_set_ref.ok_or_else(|| missing(ctx.file, block, address, "shared_set"))?;
    let keyword = keyword.ok_or_else(|| missing(ctx.file, block, address, "keyword"))?;
    Ok(JsonSharedCriterion {
        id: address.to_string(),
        shared_set,
        keyword,
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
            "campaign" => {
                campaign_ref = extract_resource_ref(ctx, &a.value)
                    .map(|r| ctx.resolve_ref(&r))
                    .or_else(|| expect_string_owned(ctx, a));
            }
            "shared_set" => {
                shared_set_ref = extract_resource_ref(ctx, &a.value)
                    .map(|r| ctx.resolve_ref(&r))
                    .or_else(|| expect_string_owned(ctx, a));
            }
            "status" => status = expect_string_owned(ctx, a),
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

fn import_proximity(ctx: &Ctx, block: &Block) -> Option<JsonProximity> {
    let mut radius = None;
    let mut units = None;
    let mut latitude = None;
    let mut longitude = None;
    for s in block.body.iter() {
        if let Structure::Attribute(a) = s {
            match a.key.as_str() {
                "latitude" => latitude = expect_f64(ctx, a),
                "longitude" => longitude = expect_f64(ctx, a),
                "radius" => radius = expect_f64(ctx, a),
                "radius_units" => units = expect_string_owned(ctx, a),
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

fn import_device(ctx: &Ctx, block: &Block) -> Option<JsonDevice> {
    let mut ty = None;
    for s in block.body.iter() {
        if let Structure::Attribute(a) = s {
            if a.key.as_str() == "type" {
                ty = expect_string_owned(ctx, a);
            }
        }
    }
    Some(JsonDevice { ty: ty? })
}

fn expect_string(ctx: &Ctx, attr: &Attribute, diags: &mut Vec<Diag>) -> Option<String> {
    if let Expression::String(s) = ctx.resolve_value(&attr.value).as_ref() {
        Some(s.as_str().to_string())
    } else {
        diags.push(Diag::new(
            ctx.file.src.clone(),
            span_of(attr.key.span()),
            format!("expected string value for '{}'", attr.key.as_str()),
        ));
        None
    }
}

fn expect_string_owned(ctx: &Ctx, attr: &Attribute) -> Option<String> {
    if let Expression::String(s) = ctx.resolve_value(&attr.value).as_ref() {
        Some(s.as_str().to_string())
    } else {
        None
    }
}

fn expect_i64(ctx: &Ctx, attr: &Attribute) -> Option<i64> {
    if let Expression::Number(n) = ctx.resolve_value(&attr.value).as_ref() {
        n.as_i64().or_else(|| {
            let f = n.as_f64()?;
            (f.is_finite() && f.fract() == 0.0 && f.abs() < 2f64.powi(53)).then_some(f as i64)
        })
    } else {
        None
    }
}

fn expect_f64(ctx: &Ctx, attr: &Attribute) -> Option<f64> {
    if let Expression::Number(n) = ctx.resolve_value(&attr.value).as_ref() {
        n.as_f64()
    } else {
        None
    }
}

fn expect_bool(ctx: &Ctx, attr: &Attribute) -> Option<bool> {
    if let Expression::Bool(b) = ctx.resolve_value(&attr.value).as_ref() {
        Some(*b.as_ref())
    } else {
        None
    }
}

fn expect_string_list(ctx: &Ctx, value: &Expression) -> Vec<String> {
    let resolved = ctx.resolve_value(value);
    let Expression::Array(arr) = resolved.as_ref() else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|item| {
            if let Expression::String(s) = ctx.resolve_value(item).as_ref() {
                Some(s.as_str().to_string())
            } else {
                None
            }
        })
        .collect()
}

fn extract_resource_ref(ctx: &Ctx, value: &Expression) -> Option<String> {
    let resolved = ctx.resolve_value(value);
    let Expression::Traversal(t) = resolved.as_ref() else {
        return None;
    };
    let path = extract_traversal_path(t)?;
    if path.len() < 2 {
        return None;
    }
    Some(format!("{}.{}", path[0], path[1]))
}

fn extract_resource_ref_list(ctx: &Ctx, value: &Expression) -> Vec<String> {
    let resolved = ctx.resolve_value(value);
    let Expression::Array(arr) = resolved.as_ref() else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|item| extract_resource_ref(ctx, item))
        .collect()
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

pub fn import_program(program: &Program) -> Result<ImportResult, Vec<Diag>> {
    let mut combined = ExportInput {
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
        sitelink_assets: Vec::new(),
        callout_assets: Vec::new(),
        structured_snippet_assets: Vec::new(),
        customer_assets: Vec::new(),
        campaign_assets: Vec::new(),
        ad_group_assets: Vec::new(),
        shared_sets: Vec::new(),
        shared_criteria: Vec::new(),
        campaign_shared_sets: Vec::new(),
        youtube_video_assets: Vec::new(),
        custom_audiences: Vec::new(),
        labels: Default::default(),
        claim_labels: Default::default(),
        campaign_claims: Default::default(),
        ad_group_claims: Default::default(),
    };
    let mut skipped: Vec<(String, String)> = Vec::new();
    let mut diags: Vec<Diag> = Vec::new();

    for (idx, scope) in program.scopes.iter().enumerate() {
        let is_top = idx == 0;
        match import_files(&scope.files, &scope.inputs) {
            Ok(r) => {
                if is_top {
                    combined.customer_id = r.input.customer_id;
                    combined.login_customer_id = r.input.login_customer_id;
                }
                combined.campaign_budgets.extend(r.input.campaign_budgets);
                combined.campaigns.extend(r.input.campaigns);
                combined.ad_groups.extend(r.input.ad_groups);
                combined.ad_group_ads.extend(r.input.ad_group_ads);
                combined.ad_group_criteria.extend(r.input.ad_group_criteria);
                combined.campaign_criteria.extend(r.input.campaign_criteria);
                combined.conversion_actions.extend(r.input.conversion_actions);
                combined.call_assets.extend(r.input.call_assets);
                combined.sitelink_assets.extend(r.input.sitelink_assets);
                combined.callout_assets.extend(r.input.callout_assets);
                combined
                    .structured_snippet_assets
                    .extend(r.input.structured_snippet_assets);
                combined.customer_assets.extend(r.input.customer_assets);
                combined.campaign_assets.extend(r.input.campaign_assets);
                combined.ad_group_assets.extend(r.input.ad_group_assets);
                combined.shared_sets.extend(r.input.shared_sets);
                combined.shared_criteria.extend(r.input.shared_criteria);
                combined.campaign_shared_sets.extend(r.input.campaign_shared_sets);
                combined.youtube_video_assets.extend(r.input.youtube_video_assets);
                combined.custom_audiences.extend(r.input.custom_audiences);
                skipped.extend(r.skipped);
            }
            Err(ds) => diags.extend(ds),
        }
    }

    if !diags.is_empty() {
        return Err(diags);
    }
    Ok(ImportResult {
        input: combined,
        skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_file;
    use std::io::Write;

    fn import_str(name: &str, content: &str) -> ExportInput {
        let mut tmp = std::env::temp_dir();
        tmp.push(format!("bidsmith-import-test-{name}.bid"));
        {
            let mut f = std::fs::File::create(&tmp).expect("create tmp");
            f.write_all(content.as_bytes()).expect("write tmp");
        }
        let pf = parse_file(&tmp).expect("parse");
        import_files(std::slice::from_ref(&pf), &InputBindings::default())
            .expect("import")
            .input
    }

    fn keyword_set(criteria: &[JsonAdGroupCriterion]) -> Vec<(String, String, bool)> {
        let mut v: Vec<(String, String, bool)> = criteria
            .iter()
            .map(|c| {
                (
                    c.keyword.text.clone(),
                    c.keyword.match_type.clone(),
                    c.negative.unwrap_or(false),
                )
            })
            .collect();
        v.sort();
        v
    }

    #[test]
    fn compact_keywords_fan_out_match_types() {
        let input = import_str(
            "fanout",
            r#"
resource "google_ads_ad_group_criterion" "kw" {
  ad_group = google_ads_ad_group.ag.id
  status   = "ENABLED"

  keywords {
    match_types = ["EXACT", "PHRASE"]
    texts       = ["a", "b", "c"]
  }
}
"#,
        );
        assert_eq!(input.ad_group_criteria.len(), 6);
        assert!(input.ad_group_criteria.iter().all(|c| c.negative == Some(false)));
        let mut got = keyword_set(&input.ad_group_criteria);
        let mut want = vec![
            ("a".to_string(), "EXACT".to_string(), false),
            ("b".to_string(), "EXACT".to_string(), false),
            ("c".to_string(), "EXACT".to_string(), false),
            ("a".to_string(), "PHRASE".to_string(), false),
            ("b".to_string(), "PHRASE".to_string(), false),
            ("c".to_string(), "PHRASE".to_string(), false),
        ];
        got.sort();
        want.sort();
        assert_eq!(got, want);
    }

    #[test]
    fn compact_form_matches_verbose_form() {
        let compact = import_str(
            "compact",
            r#"
resource "google_ads_ad_group_criterion" "kw" {
  ad_group = google_ads_ad_group.ag.id
  status   = "ENABLED"

  keywords {
    match_type = "EXACT"
    texts      = ["running shoes", "trail shoes"]
  }
}
"#,
        );
        let verbose = import_str(
            "verbose",
            r#"
resource "google_ads_ad_group_criterion" "kw" {
  ad_group = google_ads_ad_group.ag.id
  status   = "ENABLED"

  keyword {
    text       = "running shoes"
    match_type = "EXACT"
  }

  keyword {
    text       = "trail shoes"
    match_type = "EXACT"
  }
}
"#,
        );
        assert_eq!(
            keyword_set(&compact.ad_group_criteria),
            keyword_set(&verbose.ad_group_criteria)
        );
    }

    #[test]
    fn imports_youtube_video_asset_and_video_ad() {
        let input = import_str(
            "video",
            r#"
resource "google_ads_youtube_video_asset" "brand" {
  youtube_video_id    = "dQw4w9WgXcQ"
  youtube_video_title = "Brand 12s"
}

resource "google_ads_ad_group_ad" "preroll" {
  ad_group = google_ads_ad_group.ig.id

  ad {
    name       = "Preroll"
    final_urls = ["https://example.com"]

    video_responsive_ad {
      video           = google_ads_youtube_video_asset.brand.id
      headlines       = ["Block Ads"]
      call_to_actions = ["Install"]
    }
  }
}
"#,
        );
        assert_eq!(input.youtube_video_assets.len(), 1);
        let asset = &input.youtube_video_assets[0];
        assert_eq!(asset.youtube_video_id, "dQw4w9WgXcQ");
        assert_eq!(asset.youtube_video_title.as_deref(), Some("Brand 12s"));

        assert_eq!(input.ad_group_ads.len(), 1);
        let video = input.ad_group_ads[0]
            .ad
            .video_responsive_ad
            .as_ref()
            .expect("video ad body imported");
        // The `video` ref resolves to the asset's qualified address.
        assert!(
            video.video.ends_with("google_ads_youtube_video_asset.brand"),
            "video ref was {}",
            video.video
        );
        assert_eq!(video.headlines, vec!["Block Ads".to_string()]);
        assert_eq!(video.call_to_actions, vec!["Install".to_string()]);
        // A video ad carries no RSA.
        assert!(input.ad_group_ads[0].ad.responsive_search_ad.is_none());
    }

    #[test]
    fn imports_demand_gen_video_ad() {
        let input = import_str(
            "dg",
            r#"
resource "google_ads_youtube_video_asset" "shorts" {
  youtube_video_id = "dQw4w9WgXcQ"
}

resource "google_ads_ad_group_ad" "shorts_ad" {
  ad_group = google_ads_ad_group.dg.id

  ad {
    name       = "Ad 1"
    final_urls = ["https://example.com"]

    demand_gen_video_responsive_ad {
      videos         = [google_ads_youtube_video_asset.shorts.id]
      headlines      = ["Block Ads & Trackers"]
      long_headlines = ["Block ads and trackers everywhere"]
      descriptions   = ["Install the free extension."]
      breadcrumb1    = "Adblocker"
      breadcrumb2    = "Browser"
    }
  }
}
"#,
        );

        let dg = input.ad_group_ads[0]
            .ad
            .demand_gen_video_responsive_ad
            .as_ref()
            .expect("demand gen ad body imported");
        assert_eq!(dg.videos.len(), 1);
        assert!(
            dg.videos[0].ends_with("google_ads_youtube_video_asset.shorts"),
            "video ref was {}",
            dg.videos[0]
        );
        assert_eq!(dg.headlines, vec!["Block Ads & Trackers".to_string()]);
        assert_eq!(dg.long_headlines, vec!["Block ads and trackers everywhere".to_string()]);
        assert_eq!(dg.breadcrumb1.as_deref(), Some("Adblocker"));
        assert_eq!(dg.breadcrumb2.as_deref(), Some("Browser"));
        // A demand gen ad carries no RSA or plain video ad.
        assert!(input.ad_group_ads[0].ad.responsive_search_ad.is_none());
        assert!(input.ad_group_ads[0].ad.video_responsive_ad.is_none());
    }

    #[test]
    fn compact_negative_keywords_in_ad_group() {
        let input = import_str(
            "ag_neg",
            r#"
resource "google_ads_ad_group_criterion" "neg" {
  ad_group = google_ads_ad_group.ag.id
  status   = "ENABLED"

  negative_keywords {
    match_type = "BROAD"
    texts      = ["free", "cheap"]
  }
}
"#,
        );
        assert_eq!(input.ad_group_criteria.len(), 2);
        assert!(input.ad_group_criteria.iter().all(|c| c.negative == Some(true)));
    }

    #[test]
    fn compact_negative_keywords_in_campaign() {
        let input = import_str(
            "camp_neg",
            r#"
resource "google_ads_campaign_criterion" "neg" {
  campaign = google_ads_campaign.c.id
  status   = "ENABLED"

  negative_keywords {
    match_types = ["PHRASE", "EXACT"]
    texts       = ["jobs", "salary"]
  }
}
"#,
        );
        assert_eq!(input.campaign_criteria.len(), 4);
        assert!(input.campaign_criteria.iter().all(|c| c.negative == Some(true)));
        assert!(input.campaign_criteria.iter().all(|c| c.keyword.is_some()));
    }

    #[test]
    fn inline_languages_locations_expand_to_positive_criteria() {
        let input = import_str(
            "inline_targeting",
            r#"
resource "google_ads_campaign_budget" "b" {
  name          = "B"
  amount_micros = 1000000
}

resource "google_ads_campaign" "c" {
  name                     = "C"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.b.id
  languages                = ["en", "pl"]
  locations                = ["US", "geoTargetConstants/2702"]
}
"#,
        );
        assert_eq!(input.campaign_criteria.len(), 4);
        let mut constants: Vec<String> = input
            .campaign_criteria
            .iter()
            .map(|c| {
                assert_eq!(c.negative, Some(false));
                assert_eq!(c.status.as_deref(), Some("ENABLED"));
                if let Some(l) = &c.location {
                    l.geo_target_constant.clone()
                } else if let Some(l) = &c.language {
                    l.language_constant.clone()
                } else {
                    panic!("expected a location or language criterion")
                }
            })
            .collect();
        constants.sort();
        assert_eq!(
            constants,
            vec![
                "geoTargetConstants/2702".to_string(),
                "geoTargetConstants/2840".to_string(),
                "languageConstants/1000".to_string(),
                "languageConstants/1030".to_string(),
            ]
        );
        // Every expanded criterion targets the campaign's address.
        let camp = &input.campaigns[0].id;
        assert!(input.campaign_criteria.iter().all(|c| &c.campaign == camp));
    }

    #[test]
    fn inline_targeting_round_trips_against_explicit_live_state() {
        let declared = import_str(
            "inline_roundtrip",
            r#"
resource "google_ads_campaign_budget" "b" {
  name          = "B"
  amount_micros = 1000000
}

resource "google_ads_campaign" "c" {
  name                              = "C"
  status                            = "ENABLED"
  advertising_channel_type          = "SEARCH"
  campaign_budget                   = google_ads_campaign_budget.b.id
  contains_eu_political_advertising = "DOES_NOT_CONTAIN_EU_POLITICAL_ADVERTISING"
  languages                         = ["pl"]
  locations                         = ["US"]
}
"#,
        );

        // Live state as Google Ads would return it: the campaign already has the
        // two positive criteria as explicit resources.
        let live = crate::commands::adapt::from_search_response(
            r#"[{"results":[
              {"campaignBudget":{"resourceName":"customers/9/campaignBudgets/1","id":"1","name":"B","amountMicros":"1000000"}},
              {"campaign":{"resourceName":"customers/9/campaigns/2","id":"2","name":"C","status":"ENABLED","advertisingChannelType":"SEARCH","campaignBudget":"customers/9/campaignBudgets/1","containsEuPoliticalAdvertising":"DOES_NOT_CONTAIN_EU_POLITICAL_ADVERTISING"}},
              {"campaignCriterion":{"resourceName":"customers/9/campaignCriteria/2~2840","campaign":"customers/9/campaigns/2","status":"ENABLED","negative":false,"location":{"geoTargetConstant":"geoTargetConstants/2840"}}},
              {"campaignCriterion":{"resourceName":"customers/9/campaignCriteria/2~1030","campaign":"customers/9/campaigns/2","status":"ENABLED","negative":false,"language":{"languageConstant":"languageConstants/1030"}}}
            ]}]"#,
        )
        .expect("adapt live");

        let report = crate::api::diff::diff(&declared, &live);
        assert_eq!(report.create_count, 0, "diffs: {:?}", report.diffs);
        assert_eq!(report.update_count, 0, "diffs: {:?}", report.diffs);
    }

    #[test]
    fn omitted_default_status_round_trips_against_enabled_live() {
        let mut declared = import_str(
            "omit_status",
            r#"
resource "google_ads_campaign_budget" "b" {
  name          = "B"
  amount_micros = 1000000
}

resource "google_ads_campaign" "c" {
  name                     = "C"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.b.id
}
"#,
        );
        // status / contains_eu omitted in the file.
        assert!(declared.campaigns[0].status.is_none());
        declared.apply_schema_defaults();
        assert_eq!(declared.campaigns[0].status.as_deref(), Some("ENABLED"));

        let mut live = crate::commands::adapt::from_search_response(
            r#"[{"results":[
              {"campaignBudget":{"resourceName":"customers/9/campaignBudgets/1","id":"1","name":"B","amountMicros":"1000000"}},
              {"campaign":{"resourceName":"customers/9/campaigns/2","id":"2","name":"C","status":"ENABLED","advertisingChannelType":"SEARCH","campaignBudget":"customers/9/campaignBudgets/1","containsEuPoliticalAdvertising":"DOES_NOT_CONTAIN_EU_POLITICAL_ADVERTISING"}}
            ]}]"#,
        )
        .expect("adapt live");
        live.apply_schema_defaults();

        let report = crate::api::diff::diff(&declared, &live);
        assert_eq!(report.create_count, 0, "diffs: {:?}", report.diffs);
        assert_eq!(report.update_count, 0, "diffs: {:?}", report.diffs);
    }

    #[test]
    fn omitted_default_status_surfaces_drift_when_live_differs() {
        let mut declared = import_str(
            "omit_status_drift",
            r#"
resource "google_ads_campaign_budget" "b" {
  name          = "B"
  amount_micros = 1000000
}

resource "google_ads_campaign" "c" {
  name                     = "C"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.b.id
}
"#,
        );
        declared.apply_schema_defaults();

        // Someone paused the campaign in the UI; omission must enforce ENABLED.
        let mut live = crate::commands::adapt::from_search_response(
            r#"[{"results":[
              {"campaignBudget":{"resourceName":"customers/9/campaignBudgets/1","id":"1","name":"B","amountMicros":"1000000"}},
              {"campaign":{"resourceName":"customers/9/campaigns/2","id":"2","name":"C","status":"PAUSED","advertisingChannelType":"SEARCH","campaignBudget":"customers/9/campaignBudgets/1","containsEuPoliticalAdvertising":"DOES_NOT_CONTAIN_EU_POLITICAL_ADVERTISING"}}
            ]}]"#,
        )
        .expect("adapt live");
        live.apply_schema_defaults();

        let report = crate::api::diff::diff(&declared, &live);
        assert_eq!(report.update_count, 1, "diffs: {:?}", report.diffs);
        let changed: Vec<&str> = report
            .diffs
            .iter()
            .filter_map(|d| match &d.action {
                crate::api::diff::Action::Update { changed_fields, .. } => {
                    Some(changed_fields.iter().map(String::as_str).collect::<Vec<_>>())
                }
                _ => None,
            })
            .flatten()
            .collect();
        assert!(changed.contains(&"status"), "changed: {changed:?}");
    }

    #[test]
    fn omitted_negative_round_trips_for_positive_criterion() {
        // The #15 case: a positive keyword criterion that omits `negative`
        // must not churn against live state where the API reports negative=false.
        let mut declared = import_str(
            "omit_negative",
            r#"
resource "google_ads_ad_group_criterion" "kw" {
  ad_group = google_ads_ad_group.ag.id

  keyword {
    text       = "running shoes"
    match_type = "EXACT"
  }
}
"#,
        );
        declared.apply_schema_defaults();
        assert_eq!(declared.ad_group_criteria[0].negative, Some(false));
        assert_eq!(declared.ad_group_criteria[0].status.as_deref(), Some("ENABLED"));
    }

    #[test]
    fn compact_negative_keywords_in_shared_set() {
        let input = import_str(
            "shared",
            r#"
resource "google_ads_shared_set" "brands" {
  name = "Brands"

  negative_keywords {
    match_type = "BROAD"
    texts      = ["acme", "globex", "initech"]
  }
}
"#,
        );
        assert_eq!(input.shared_criteria.len(), 3);
        assert!(input.shared_sets[0].negative_keywords.is_empty());
    }

    #[test]
    fn rsa_list_attributes_resolve_from_locals() {
        let input = import_str(
            "rsa_list_local",
            r#"
locals {
  headlines = [
    "First Headline",
    "Second Headline",
    { text = "Pinned Headline", pin = "HEADLINE_1" },
  ]
  descriptions = ["First description", "Second description"]
  urls         = ["https://example.com/landing"]
}

resource "google_ads_ad_group_ad" "rsa" {
  ad_group = google_ads_ad_group.ag.id
  status   = "ENABLED"

  ad {
    final_urls = local.urls

    responsive_search_ad {
      headlines    = local.headlines
      descriptions = local.descriptions
    }
  }
}
"#,
        );
        let ad = &input.ad_group_ads[0].ad;
        assert_eq!(ad.final_urls, vec!["https://example.com/landing".to_string()]);
        let rsa = ad.responsive_search_ad.as_ref().expect("rsa present");
        let headlines: Vec<(&str, Option<&str>)> = rsa
            .headlines
            .iter()
            .map(|h| (h.text.as_str(), h.pin.as_deref()))
            .collect();
        assert_eq!(
            headlines,
            vec![
                ("First Headline", None),
                ("Second Headline", None),
                ("Pinned Headline", Some("HEADLINE_1")),
            ]
        );
        let descriptions: Vec<&str> = rsa.descriptions.iter().map(|d| d.text.as_str()).collect();
        assert_eq!(descriptions, vec!["First description", "Second description"]);
    }

    #[test]
    fn ad_template_expands_into_each_referencing_ad() {
        let input = import_str(
            "ad_template",
            r#"
ad_template "shared" {
  final_urls = ["https://example.com/landing"]
  responsive_search_ad {
    headlines    = ["First Headline", "Second Headline", "Third Headline"]
    descriptions = ["First description", "Second description"]
    path1        = "shop"
  }
}

resource "google_ads_ad_group_ad" "a" {
  ad_group = google_ads_ad_group.ag_a.id
  status   = "ENABLED"
  template = ad_template.shared
}

resource "google_ads_ad_group_ad" "b" {
  ad_group = google_ads_ad_group.ag_b.id
  status   = "ENABLED"
  template = ad_template.shared
}
"#,
        );
        assert_eq!(input.ad_group_ads.len(), 2);
        for ad in &input.ad_group_ads {
            assert_eq!(ad.ad.final_urls, vec!["https://example.com/landing".to_string()]);
            let rsa = ad.ad.responsive_search_ad.as_ref().expect("rsa present");
            let headlines: Vec<&str> = rsa.headlines.iter().map(|h| h.text.as_str()).collect();
            assert_eq!(headlines, vec!["First Headline", "Second Headline", "Third Headline"]);
            assert_eq!(rsa.descriptions.len(), 2);
            assert_eq!(rsa.path1.as_deref(), Some("shop"));
        }
        // The two ads keep their own distinct per-ad-group addresses.
        let ids: Vec<&str> = input.ad_group_ads.iter().map(|a| a.id.as_str()).collect();
        assert!(ids.iter().any(|id| id.ends_with("google_ads_ad_group_ad.a")));
        assert!(ids.iter().any(|id| id.ends_with("google_ads_ad_group_ad.b")));
    }

    #[test]
    fn ad_template_overrides_apply_per_instance() {
        let input = import_str(
            "ad_template_overrides",
            r#"
ad_template "shared" {
  final_urls = ["https://example.com/default"]
  responsive_search_ad {
    headlines    = ["First Headline", "Second Headline", "Third Headline"]
    descriptions = ["First description", "Second description"]
    path1        = "default"
    path2        = "shop"
  }
}

resource "google_ads_ad_group_ad" "base" {
  ad_group = google_ads_ad_group.ag_base.id
  template = ad_template.shared
}

resource "google_ads_ad_group_ad" "custom" {
  ad_group   = google_ads_ad_group.ag_custom.id
  template   = ad_template.shared
  final_urls = ["https://example.com/custom"]
  path1      = "custom"
}
"#,
        );
        let by_addr = |suffix: &str| {
            input
                .ad_group_ads
                .iter()
                .find(|a| a.id.ends_with(suffix))
                .unwrap_or_else(|| panic!("missing {suffix}"))
        };

        // No overrides → the template body is used verbatim.
        let base = &by_addr("google_ads_ad_group_ad.base").ad;
        assert_eq!(base.final_urls, vec!["https://example.com/default".to_string()]);
        let base_rsa = base.responsive_search_ad.as_ref().expect("rsa");
        assert_eq!(base_rsa.path1.as_deref(), Some("default"));
        assert_eq!(base_rsa.path2.as_deref(), Some("shop"));

        // Overrides win for the fields they set; unset fields (descriptions, path2,
        // headlines) inherit from the template.
        let custom = &by_addr("google_ads_ad_group_ad.custom").ad;
        assert_eq!(custom.final_urls, vec!["https://example.com/custom".to_string()]);
        let custom_rsa = custom.responsive_search_ad.as_ref().expect("rsa");
        assert_eq!(custom_rsa.path1.as_deref(), Some("custom"));
        assert_eq!(custom_rsa.path2.as_deref(), Some("shop"));
        let headlines: Vec<&str> = custom_rsa.headlines.iter().map(|h| h.text.as_str()).collect();
        assert_eq!(headlines, vec!["First Headline", "Second Headline", "Third Headline"]);
        assert_eq!(custom_rsa.descriptions.len(), 2);
    }

    fn diff_after_defaults(
        mut declared: ExportInput,
        mut live: ExportInput,
    ) -> crate::api::diff::DiffReport {
        declared.apply_schema_defaults();
        live.apply_schema_defaults();
        crate::api::diff::diff(&declared, &live)
    }

    fn delete_live_ids(report: &crate::api::diff::DiffReport) -> Vec<String> {
        report
            .diffs
            .iter()
            .filter_map(|d| match &d.action {
                crate::api::diff::Action::Delete { live_id } => Some(live_id.clone()),
                _ => None,
            })
            .collect()
    }

    fn delete_addresses(report: &crate::api::diff::DiffReport) -> Vec<String> {
        report
            .diffs
            .iter()
            .filter(|d| matches!(d.action, crate::api::diff::Action::Delete { .. }))
            .map(|d| d.address.clone())
            .collect()
    }

    #[test]
    fn removing_an_ad_group_negative_plans_a_delete() {
        // The #43 case: one negative_keyword block is dropped from a resource
        // that keeps its other blocks. The dropped member must plan as a
        // delete; a live positive keyword nobody declared is left alone.
        let declared = import_str(
            "agc_prune",
            r#"
resource "google_ads_campaign_budget" "b" {
  name          = "B"
  amount_micros = 1000000
}

resource "google_ads_campaign" "c" {
  name                     = "C"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.b.id
}

resource "google_ads_ad_group" "ag" {
  name     = "AG"
  campaign = google_ads_campaign.c.id
}

resource "google_ads_ad_group_criterion" "neg" {
  ad_group = google_ads_ad_group.ag.id

  negative_keywords {
    match_type = "BROAD"
    texts      = ["free"]
  }
}
"#,
        );
        let live = crate::commands::adapt::from_search_response(
            r#"[{"results":[
              {"campaignBudget":{"resourceName":"customers/9/campaignBudgets/1","id":"1","name":"B","amountMicros":"1000000"}},
              {"campaign":{"resourceName":"customers/9/campaigns/2","id":"2","name":"C","status":"ENABLED","advertisingChannelType":"SEARCH","campaignBudget":"customers/9/campaignBudgets/1","containsEuPoliticalAdvertising":"DOES_NOT_CONTAIN_EU_POLITICAL_ADVERTISING"}},
              {"adGroup":{"resourceName":"customers/9/adGroups/3","id":"3","name":"AG","status":"ENABLED","campaign":"customers/9/campaigns/2"}},
              {"adGroupCriterion":{"resourceName":"customers/9/adGroupCriteria/3~100","adGroup":"customers/9/adGroups/3","status":"ENABLED","negative":true,"keyword":{"text":"free","matchType":"BROAD"}}},
              {"adGroupCriterion":{"resourceName":"customers/9/adGroupCriteria/3~101","adGroup":"customers/9/adGroups/3","status":"ENABLED","negative":true,"keyword":{"text":"cheap","matchType":"BROAD"}}},
              {"adGroupCriterion":{"resourceName":"customers/9/adGroupCriteria/3~102","adGroup":"customers/9/adGroups/3","status":"ENABLED","negative":false,"keyword":{"text":"shoes","matchType":"EXACT"}}}
            ]}]"#,
        )
        .expect("adapt live");

        let report = diff_after_defaults(declared, live);
        assert_eq!(report.create_count, 0, "diffs: {:?}", report.diffs);
        assert_eq!(report.update_count, 0, "diffs: {:?}", report.diffs);
        assert_eq!(report.delete_count, 1, "diffs: {:?}", report.diffs);
        assert_eq!(delete_live_ids(&report), vec!["3~101".to_string()]);
        let addrs = delete_addresses(&report);
        assert!(addrs[0].contains("cheap"), "delete row: {addrs:?}");
        // The unmanaged positive keyword must not be deleted.
        assert!(
            !addrs.iter().any(|a| a.contains("shoes")),
            "a live positive nobody declared was pruned: {addrs:?}"
        );
    }

    #[test]
    fn removing_a_campaign_negative_plans_a_delete_but_spares_locations() {
        let declared = import_str(
            "camp_prune",
            r#"
resource "google_ads_campaign_budget" "b" {
  name          = "B"
  amount_micros = 1000000
}

resource "google_ads_campaign" "c" {
  name                     = "C"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.b.id
}

resource "google_ads_campaign_criterion" "neg" {
  campaign = google_ads_campaign.c.id

  negative_keywords {
    match_types = ["PHRASE"]
    texts       = ["jobs"]
  }
}
"#,
        );
        let live = crate::commands::adapt::from_search_response(
            r#"[{"results":[
              {"campaignBudget":{"resourceName":"customers/9/campaignBudgets/1","id":"1","name":"B","amountMicros":"1000000"}},
              {"campaign":{"resourceName":"customers/9/campaigns/2","id":"2","name":"C","status":"ENABLED","advertisingChannelType":"SEARCH","campaignBudget":"customers/9/campaignBudgets/1","containsEuPoliticalAdvertising":"DOES_NOT_CONTAIN_EU_POLITICAL_ADVERTISING"}},
              {"campaignCriterion":{"resourceName":"customers/9/campaignCriteria/2~500","campaign":"customers/9/campaigns/2","status":"ENABLED","negative":true,"keyword":{"text":"jobs","matchType":"PHRASE"}}},
              {"campaignCriterion":{"resourceName":"customers/9/campaignCriteria/2~501","campaign":"customers/9/campaigns/2","status":"ENABLED","negative":true,"keyword":{"text":"salary","matchType":"PHRASE"}}},
              {"campaignCriterion":{"resourceName":"customers/9/campaignCriteria/2~2840","campaign":"customers/9/campaigns/2","status":"ENABLED","negative":false,"location":{"geoTargetConstant":"geoTargetConstants/2840"}}}
            ]}]"#,
        )
        .expect("adapt live");

        let report = diff_after_defaults(declared, live);
        assert_eq!(report.delete_count, 1, "diffs: {:?}", report.diffs);
        assert_eq!(delete_live_ids(&report), vec!["2~501".to_string()]);
        let addrs = delete_addresses(&report);
        assert!(addrs[0].contains("salary"), "delete row: {addrs:?}");
        // A location criterion nobody declared (no declared location category)
        // must survive.
        assert!(
            !addrs.iter().any(|a| a.contains("location")),
            "an undeclared location was pruned: {addrs:?}"
        );
    }

    #[test]
    fn removing_a_shared_set_member_plans_a_delete() {
        let declared = import_str(
            "shared_prune",
            r#"
resource "google_ads_shared_set" "s" {
  name = "Brands"

  negative_keywords {
    match_type = "BROAD"
    texts      = ["acme"]
  }
}
"#,
        );
        let live = crate::commands::adapt::from_search_response(
            r#"[{"results":[
              {"sharedSet":{"resourceName":"customers/9/sharedSets/50","id":"50","name":"Brands","type":"NEGATIVE_KEYWORDS","status":"ENABLED"}},
              {"sharedCriterion":{"resourceName":"customers/9/sharedCriteria/50~200","sharedSet":"customers/9/sharedSets/50","keyword":{"text":"acme","matchType":"BROAD"}}},
              {"sharedCriterion":{"resourceName":"customers/9/sharedCriteria/50~201","sharedSet":"customers/9/sharedSets/50","keyword":{"text":"globex","matchType":"BROAD"}}}
            ]}]"#,
        )
        .expect("adapt live");

        let report = diff_after_defaults(declared, live);
        assert_eq!(report.delete_count, 1, "diffs: {:?}", report.diffs);
        assert_eq!(delete_live_ids(&report), vec!["50~201".to_string()]);
    }

    #[test]
    fn criteria_under_an_undeclared_parent_are_not_pruned() {
        // The ad group itself isn't declared, so bidsmith doesn't own its
        // criteria — nothing here should plan as a delete (that whole-resource
        // case waits on identity labels).
        let declared = import_str(
            "no_parent_prune",
            r#"
resource "google_ads_campaign_budget" "b" {
  name          = "B"
  amount_micros = 1000000
}

resource "google_ads_campaign" "c" {
  name                     = "C"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.b.id
}
"#,
        );
        let live = crate::commands::adapt::from_search_response(
            r#"[{"results":[
              {"campaignBudget":{"resourceName":"customers/9/campaignBudgets/1","id":"1","name":"B","amountMicros":"1000000"}},
              {"campaign":{"resourceName":"customers/9/campaigns/2","id":"2","name":"C","status":"ENABLED","advertisingChannelType":"SEARCH","campaignBudget":"customers/9/campaignBudgets/1","containsEuPoliticalAdvertising":"DOES_NOT_CONTAIN_EU_POLITICAL_ADVERTISING"}},
              {"adGroup":{"resourceName":"customers/9/adGroups/3","id":"3","name":"AG","status":"ENABLED","campaign":"customers/9/campaigns/2"}},
              {"adGroupCriterion":{"resourceName":"customers/9/adGroupCriteria/3~100","adGroup":"customers/9/adGroups/3","status":"ENABLED","negative":true,"keyword":{"text":"free","matchType":"BROAD"}}}
            ]}]"#,
        )
        .expect("adapt live");

        let report = diff_after_defaults(declared, live);
        assert_eq!(report.delete_count, 0, "diffs: {:?}", report.diffs);
    }

    #[test]
    fn compact_keyword_texts_resolve_from_locals() {
        let input = import_str(
            "kw_texts_local",
            r#"
locals {
  themes = ["ublock", "ublock origin", "adblock alternative"]
}

resource "google_ads_ad_group_criterion" "kw" {
  ad_group = google_ads_ad_group.ag.id
  status   = "ENABLED"

  keywords {
    texts      = local.themes
    match_type = "PHRASE"
  }
}
"#,
        );
        assert_eq!(input.ad_group_criteria.len(), 3);
        let got = keyword_set(&input.ad_group_criteria);
        assert_eq!(
            got,
            vec![
                ("adblock alternative".to_string(), "PHRASE".to_string(), false),
                ("ublock".to_string(), "PHRASE".to_string(), false),
                ("ublock origin".to_string(), "PHRASE".to_string(), false),
            ]
        );
    }

    #[test]
    fn defaults_fill_missing_campaign_attributes() {
        let input = import_str(
            "defaults_merge",
            r#"
defaults "google_ads_campaign" {
  advertising_channel_type = "SEARCH"
  languages = ["en"]

  manual_cpc {
    enhanced_cpc_enabled = false
  }

  network_settings {
    target_google_search = true
    target_search_network = false
  }
}

resource "google_ads_campaign_budget" "b" {
  name          = "B"
  amount_micros = 1000000
}

resource "google_ads_campaign" "shell" {
  name            = "GH_Cookies 08.07.2026"
  campaign_budget = google_ads_campaign_budget.b.id
}
"#,
        );
        assert_eq!(input.campaigns.len(), 1);
        let c = &input.campaigns[0];
        assert_eq!(c.advertising_channel_type, "SEARCH");
        assert_eq!(
            c.manual_cpc.as_ref().and_then(|m| m.enhanced_cpc_enabled),
            Some(false)
        );
        assert_eq!(
            c.network_settings
                .as_ref()
                .and_then(|n| n.target_google_search),
            Some(true)
        );
        // `languages = ["en"]` expands to one positive language criterion.
        assert_eq!(input.campaign_criteria.len(), 1);
        assert_eq!(
            input.campaign_criteria[0]
                .language
                .as_ref()
                .map(|l| l.language_constant.as_str()),
            Some("languageConstants/1000")
        );
    }

    fn video_campaign_with_caps(name: &str, caps: &str) -> ExportInput {
        import_str(
            name,
            &format!(
                r#"
locals {{
  impression_cap = 3
}}

resource "google_ads_campaign_budget" "b" {{
  name          = "B"
  amount_micros = 1000000
}}

resource "google_ads_campaign" "v" {{
  name                     = "V"
  advertising_channel_type = "VIDEO"
  campaign_budget          = google_ads_campaign_budget.b.id
{caps}
}}
"#
            ),
        )
    }

    const LIVE_VIDEO_CAMPAIGN: &str = r#"[{"results":[
      {"campaignBudget":{"resourceName":"customers/9/campaignBudgets/1","id":"1","name":"B","amountMicros":"1000000"}},
      {"campaign":{"resourceName":"customers/9/campaigns/2","id":"2","name":"V","status":"ENABLED","advertisingChannelType":"VIDEO","campaignBudget":"customers/9/campaignBudgets/1","containsEuPoliticalAdvertising":"DOES_NOT_CONTAIN_EU_POLITICAL_ADVERTISING","frequencyCaps":[
        {"key":{"level":"CAMPAIGN","eventType":"IMPRESSION","timeUnit":"DAY","timeLength":1},"cap":3},
        {"key":{"level":"CAMPAIGN","eventType":"VIDEO_VIEW","timeUnit":"DAY","timeLength":1},"cap":1}
      ]}}
    ]}]"#;

    #[test]
    fn frequency_caps_import_as_a_list_and_match_live_in_any_order() {
        let declared = video_campaign_with_caps(
            "freq_caps_order",
            r#"
  frequency_caps {
    event_type  = "VIDEO_VIEW"
    time_unit   = "DAY"
    time_length = 1
    cap         = 1
  }

  frequency_caps {
    event_type  = "IMPRESSION"
    time_unit   = "DAY"
    time_length = 1
    cap         = local.impression_cap
  }
"#,
        );
        assert_eq!(declared.campaigns[0].frequency_caps.len(), 2);
        assert_eq!(
            declared.campaigns[0].frequency_caps[0].level_or_default(),
            "CAMPAIGN"
        );

        let live = crate::commands::adapt::from_search_response(LIVE_VIDEO_CAMPAIGN)
            .expect("adapt live");
        let report = diff_after_defaults(declared, live);
        assert_eq!(report.update_count, 0, "diffs: {:?}", report.diffs);
    }

    #[test]
    fn a_changed_cap_plans_an_update() {
        let declared = video_campaign_with_caps(
            "freq_caps_changed",
            r#"
  frequency_caps {
    event_type  = "IMPRESSION"
    time_unit   = "DAY"
    time_length = 1
    cap         = 5
  }

  frequency_caps {
    event_type  = "VIDEO_VIEW"
    time_unit   = "DAY"
    time_length = 1
    cap         = 1
  }
"#,
        );
        let live = crate::commands::adapt::from_search_response(LIVE_VIDEO_CAMPAIGN)
            .expect("adapt live");
        let report = diff_after_defaults(declared, live);
        let changed: Vec<&Vec<String>> = report
            .diffs
            .iter()
            .filter_map(|d| match &d.action {
                crate::api::diff::Action::Update { changed_fields, .. } => Some(changed_fields),
                _ => None,
            })
            .collect();
        assert_eq!(changed.len(), 1, "diffs: {:?}", report.diffs);
        assert_eq!(changed[0], &vec!["frequency_caps".to_string()]);
    }

    /// `LIVE_VIDEO_CAMPAIGN` plus the `bidsmith:owns=frequency_caps`
    /// association a previous apply wrote when the file still declared caps.
    fn live_video_campaign_claiming_caps() -> ExportInput {
        let mut batches: serde_json::Value =
            serde_json::from_str(LIVE_VIDEO_CAMPAIGN).expect("live json");
        batches[0]["results"]
            .as_array_mut()
            .expect("results array")
            .push(serde_json::json!({
                "campaignLabel": {
                    "resourceName": "customers/9/campaignLabels/2~777",
                    "campaign": "customers/9/campaigns/2",
                    "label": "customers/9/labels/777"
                },
                "label": {
                    "resourceName": "customers/9/labels/777",
                    "name": "bidsmith:owns=frequency_caps"
                }
            }));
        crate::commands::adapt::from_search_response(&batches.to_string()).expect("adapt live")
    }

    fn caps_update_planned(report: &crate::api::diff::DiffReport) -> bool {
        report.diffs.iter().any(|d| {
            matches!(
                &d.action,
                crate::api::diff::Action::Update { changed_fields, .. }
                    if changed_fields.contains(&"frequency_caps".to_string())
            )
        })
    }

    #[test]
    fn caps_set_outside_bidsmith_stay_unmanaged() {
        // Issue #102: a campaign that never declared a cap doesn't own the
        // field, so UI-set caps must not plan a clear (which the API rejects
        // outright on some campaigns, poisoning the whole atomic batch).
        let declared = video_campaign_with_caps("freq_caps_none", "");
        let live = crate::commands::adapt::from_search_response(LIVE_VIDEO_CAMPAIGN)
            .expect("adapt live");
        let report = diff_after_defaults(declared, live);
        assert!(!caps_update_planned(&report), "diffs: {:?}", report.diffs);
        assert!(
            report
                .claim_plans
                .iter()
                .all(|p| p.category != "frequency_caps"),
            "an undeclared field claims nothing",
        );
    }

    #[test]
    fn declaring_a_cap_claims_the_field() {
        let declared = video_campaign_with_caps(
            "freq_caps_claimed",
            r#"
  frequency_caps {
    event_type  = "IMPRESSION"
    time_unit   = "DAY"
    time_length = 1
    cap         = 3
  }

  frequency_caps {
    event_type  = "VIDEO_VIEW"
    time_unit   = "DAY"
    time_length = 1
    cap         = 1
  }
"#,
        );
        let live = crate::commands::adapt::from_search_response(LIVE_VIDEO_CAMPAIGN)
            .expect("adapt live");
        let report = diff_after_defaults(declared, live);
        assert!(!caps_update_planned(&report), "diffs: {:?}", report.diffs);
        assert!(
            report
                .claim_plans
                .iter()
                .any(|p| p.category == "frequency_caps" && p.stale_assoc_rn.is_none()),
            "claims: {:?}",
            report.claim_plans
        );
    }

    #[test]
    fn dropping_the_last_declared_cap_clears_and_releases_the_claim() {
        let declared = video_campaign_with_caps("freq_caps_dropped", "");
        let report = diff_after_defaults(declared, live_video_campaign_claiming_caps());
        assert!(caps_update_planned(&report), "diffs: {:?}", report.diffs);
        assert!(
            report
                .claim_plans
                .iter()
                .any(|p| p.category == "frequency_caps" && p.stale_assoc_rn.is_some()),
            "claims: {:?}",
            report.claim_plans
        );
    }

    const VIDEO_TARGETING_BID: &str = r#"
resource "google_ads_custom_audience" "adblock" {
  name        = "Ad blocker searchers"
  description = "Search-intent segment"
  type        = "SEARCH"

  member { keyword = "ad blocker" }
  member { keyword = "block ads" }
}

resource "google_ads_campaign_budget" "b" {
  name          = "B"
  amount_micros = 1000000
}

resource "google_ads_campaign" "v" {
  name                     = "V"
  advertising_channel_type = "VIDEO"
  campaign_budget          = google_ads_campaign_budget.b.id
}

resource "google_ads_campaign_criterion" "intent" {
  campaign = google_ads_campaign.v.id

  audience {
    custom_audience = google_ads_custom_audience.adblock.id
  }
}

resource "google_ads_campaign_criterion" "channel" {
  campaign = google_ads_campaign.v.id

  youtube_channel { channel_id = "UCabc" }
}

resource "google_ads_campaign_criterion" "no_kids" {
  campaign = google_ads_campaign.v.id
  negative = true

  age_range { type = "AGE_RANGE_18_24" }
}
"#;

    const LIVE_VIDEO_TARGETING: &str = r#"[{"results":[
      {"customAudience":{"resourceName":"customers/9/customAudiences/501","id":"501","name":"Ad blocker searchers","description":"Search-intent segment","type":"SEARCH","status":"ENABLED","members":[
        {"memberType":"KEYWORD","keyword":"block ads"},
        {"memberType":"KEYWORD","keyword":"ad blocker"}
      ]}},
      {"campaignBudget":{"resourceName":"customers/9/campaignBudgets/1","id":"1","name":"B","amountMicros":"1000000"}},
      {"campaign":{"resourceName":"customers/9/campaigns/2","id":"2","name":"V","status":"ENABLED","advertisingChannelType":"VIDEO","campaignBudget":"customers/9/campaignBudgets/1","containsEuPoliticalAdvertising":"DOES_NOT_CONTAIN_EU_POLITICAL_ADVERTISING"}},
      {"campaignCriterion":{"resourceName":"customers/9/campaignCriteria/2~9001","campaign":"customers/9/campaigns/2","status":"ENABLED","negative":false,"customAudience":{"customAudience":"customers/9/customAudiences/501"}}},
      {"campaignCriterion":{"resourceName":"customers/9/campaignCriteria/2~9002","campaign":"customers/9/campaigns/2","status":"ENABLED","negative":false,"youtubeChannel":{"channelId":"UCabc"}}},
      {"campaignCriterion":{"resourceName":"customers/9/campaignCriteria/2~9003","campaign":"customers/9/campaigns/2","status":"ENABLED","negative":true,"ageRange":{"type":"AGE_RANGE_18_24"}}}
    ]}]"#;

    #[test]
    fn video_targeting_criteria_import_and_match_live() {
        let declared = import_str("video_targeting", VIDEO_TARGETING_BID);
        assert_eq!(declared.custom_audiences.len(), 1);
        assert_eq!(declared.custom_audiences[0].members.len(), 2);
        // The audience criterion references the declared segment by address,
        // not by a resource name it cannot know before apply.
        let audience = declared
            .campaign_criteria
            .iter()
            .find_map(|c| c.audience.as_ref())
            .expect("audience criterion");
        assert!(
            audience
                .custom_audience
                .as_deref()
                .is_some_and(|a| a.ends_with("google_ads_custom_audience.adblock")),
            "{:?}",
            audience.custom_audience
        );

        let live =
            crate::commands::adapt::from_search_response(LIVE_VIDEO_TARGETING).expect("adapt live");
        let report = diff_after_defaults(declared, live);
        assert_eq!(report.create_count, 0, "diffs: {:?}", report.diffs);
        assert_eq!(report.update_count, 0, "diffs: {:?}", report.diffs);
        assert_eq!(report.delete_count, 0, "diffs: {:?}", report.diffs);
    }

    #[test]
    fn dropping_a_placement_plans_a_delete_but_spares_other_axes() {
        // Removing the youtube_channel block leaves the campaign declaring an
        // audience and an age_range: bidsmith owns the placement category on
        // this campaign only while it declares one, so the live channel is the
        // single delete and the untouched axes stay put.
        let declared = import_str(
            "video_targeting_prune",
            &VIDEO_TARGETING_BID.replace(
                r#"resource "google_ads_campaign_criterion" "channel" {
  campaign = google_ads_campaign.v.id

  youtube_channel { channel_id = "UCabc" }
}"#,
                r#"resource "google_ads_campaign_criterion" "other_channel" {
  campaign = google_ads_campaign.v.id

  youtube_channel { channel_id = "UCdef" }
}"#,
            ),
        );
        let live =
            crate::commands::adapt::from_search_response(LIVE_VIDEO_TARGETING).expect("adapt live");
        let report = diff_after_defaults(declared, live);
        assert_eq!(report.create_count, 1, "diffs: {:?}", report.diffs);
        assert_eq!(report.delete_count, 1, "diffs: {:?}", report.diffs);
        let addrs = delete_addresses(&report);
        assert!(addrs[0].contains("youtube_channel UCabc"), "{addrs:?}");
    }

    #[test]
    fn an_edited_segment_member_plans_an_update_not_a_recreate() {
        let declared = import_str(
            "custom_audience_members",
            &VIDEO_TARGETING_BID.replace(r#"member { keyword = "block ads" }"#, ""),
        );
        let live =
            crate::commands::adapt::from_search_response(LIVE_VIDEO_TARGETING).expect("adapt live");
        let report = diff_after_defaults(declared, live);
        let changed: Vec<(&str, &Vec<String>)> = report
            .diffs
            .iter()
            .filter_map(|d| match &d.action {
                crate::api::diff::Action::Update { changed_fields, .. } => {
                    Some((d.kind, changed_fields))
                }
                _ => None,
            })
            .collect();
        assert_eq!(changed.len(), 1, "diffs: {:?}", report.diffs);
        assert_eq!(changed[0].0, "custom_audience");
        assert_eq!(changed[0].1, &vec!["members".to_string()]);
    }

    #[test]
    fn resource_attributes_win_over_defaults() {
        let input = import_str(
            "defaults_override",
            r#"
defaults "google_ads_campaign" {
  advertising_channel_type = "SEARCH"

  manual_cpc {
    enhanced_cpc_enabled = false
  }
}

resource "google_ads_campaign_budget" "b" {
  name          = "B"
  amount_micros = 1000000
}

resource "google_ads_campaign" "display" {
  name                     = "Display"
  advertising_channel_type = "DISPLAY"
  campaign_budget          = google_ads_campaign_budget.b.id

  manual_cpc {
    enhanced_cpc_enabled = true
  }
}
"#,
        );
        assert_eq!(input.campaigns.len(), 1);
        let c = &input.campaigns[0];
        assert_eq!(c.advertising_channel_type, "DISPLAY");
        assert_eq!(
            c.manual_cpc.as_ref().and_then(|m| m.enhanced_cpc_enabled),
            Some(true)
        );
    }

    #[test]
    fn for_each_fans_out_device_criteria() {
        let input = import_str(
            "fe_devices",
            r#"
resource "google_ads_campaign_criterion" "gh_cookies_device_exclusions" {
  for_each = ["MOBILE", "TABLET"]
  campaign = google_ads_campaign.gh_cookies.id
  bid_modifier = 0

  device {
    type = each.value
  }
}
"#,
        );
        assert_eq!(input.campaign_criteria.len(), 2);
        let mut got: Vec<(String, String, Option<f64>)> = input
            .campaign_criteria
            .iter()
            .map(|c| {
                (
                    c.id.clone(),
                    c.device.as_ref().expect("device").ty.clone(),
                    c.bid_modifier,
                )
            })
            .collect();
        got.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            got,
            vec![
                (
                    "bidsmith_import_test_fe_devices.google_ads_campaign_criterion.gh_cookies_device_exclusions[\"MOBILE\"]"
                        .to_string(),
                    "MOBILE".to_string(),
                    Some(0.0)
                ),
                (
                    "bidsmith_import_test_fe_devices.google_ads_campaign_criterion.gh_cookies_device_exclusions[\"TABLET\"]"
                        .to_string(),
                    "TABLET".to_string(),
                    Some(0.0)
                ),
            ]
        );
    }

    #[test]
    fn for_each_map_fans_out_campaign_assets() {
        let input = import_str(
            "fe_assets",
            r#"
resource "google_ads_sitelink_asset" "sl_neverconsent" {
  link_text  = "Never-Consent"
  final_urls = ["https://example.com/never-consent"]
}

resource "google_ads_sitelink_asset" "sl_adblock" {
  link_text  = "Ad Blocker"
  final_urls = ["https://example.com/ad-blocker"]
}

resource "google_ads_campaign_asset" "gh_cookies_sitelinks" {
  for_each = {
    neverconsent = google_ads_sitelink_asset.sl_neverconsent.id
    adblock = google_ads_sitelink_asset.sl_adblock.id
  }
  campaign = google_ads_campaign.gh_cookies.id
  asset = each.value
  field_type = "SITELINK"
}
"#,
        );
        assert_eq!(input.campaign_assets.len(), 2);
        let mut got: Vec<(String, String)> = input
            .campaign_assets
            .iter()
            .map(|a| (a.asset.clone(), a.field_type.clone()))
            .collect();
        got.sort();
        assert_eq!(
            got,
            vec![
                (
                    "bidsmith_import_test_fe_assets.google_ads_sitelink_asset.sl_adblock".to_string(),
                    "SITELINK".to_string()
                ),
                (
                    "bidsmith_import_test_fe_assets.google_ads_sitelink_asset.sl_neverconsent".to_string(),
                    "SITELINK".to_string()
                ),
            ]
        );
    }

    #[test]
    fn concat_merges_headline_lists_in_order() {
        let input = import_str(
            "concat_headlines",
            r#"
locals {
  hl_specific = ["Stop Cookie Pop-Ups for Good", { text = "Block Cookie Banners", pin = "HEADLINE_1" }]
  hl_brand_tail = ["Add to Chrome, Free", "Open Source & Private"]
  desc_brand_tail = ["Free, open source, and private."]
}

resource "google_ads_ad_group_ad" "rsa" {
  ad_group = google_ads_ad_group.g.id
  ad {
    final_urls = concat(["https://example.com/a"], ["https://example.com/b"])
    responsive_search_ad {
      headlines    = concat(local.hl_specific, local.hl_brand_tail)
      descriptions = concat(["Browse without interruptions."], local.desc_brand_tail)
    }
  }
}
"#,
        );
        assert_eq!(input.ad_group_ads.len(), 1);
        let ad = &input.ad_group_ads[0].ad;
        assert_eq!(
            ad.final_urls,
            vec![
                "https://example.com/a".to_string(),
                "https://example.com/b".to_string()
            ]
        );
        let rsa = ad.responsive_search_ad.as_ref().expect("rsa");
        let texts: Vec<&str> = rsa.headlines.iter().map(|h| h.text.as_str()).collect();
        assert_eq!(
            texts,
            vec![
                "Stop Cookie Pop-Ups for Good",
                "Block Cookie Banners",
                "Add to Chrome, Free",
                "Open Source & Private"
            ]
        );
        assert_eq!(rsa.headlines[1].pin.as_deref(), Some("HEADLINE_1"));
        let descs: Vec<&str> = rsa.descriptions.iter().map(|d| d.text.as_str()).collect();
        assert_eq!(
            descs,
            vec![
                "Browse without interruptions.",
                "Free, open source, and private."
            ]
        );
    }

    #[test]
    fn templates_render_in_scalar_attributes_and_locals() {
        let input = import_str(
            "tmpl_scalars",
            r#"
locals {
  utm  = "GH_Test_0101"
  name = "t ${local.utm}"
}

resource "google_ads_campaign_budget" "t" {
  name          = local.name
  amount_micros = 1000000
}
"#,
        );
        assert_eq!(input.campaign_budgets.len(), 1);
        assert_eq!(input.campaign_budgets[0].name, "t GH_Test_0101");
    }

    #[test]
    fn templates_render_in_final_urls_and_rsa_lists() {
        let input = import_str(
            "tmpl_urls",
            r#"
locals {
  base  = "https://www.ghostery.com/ghostery-ad-blocker?utm_source=search&utm_campaign=GH_Cookies_0708"
  brand = "Ghostery"
}

resource "google_ads_ad_group_ad" "rsa" {
  ad_group = google_ads_ad_group.g.id
  ad {
    final_urls = ["${local.base}-rsa_a"]
    responsive_search_ad {
      headlines    = ["Stop Cookie Pop-Ups", "${local.brand} Ad Blocker", { text = "Try ${local.brand} Free", pin = "HEADLINE_1" }]
      descriptions = ["A description here", "Another description here"]
    }
  }
}
"#,
        );
        assert_eq!(input.ad_group_ads.len(), 1);
        let ad = &input.ad_group_ads[0].ad;
        assert_eq!(
            ad.final_urls,
            vec![
                "https://www.ghostery.com/ghostery-ad-blocker?utm_source=search&utm_campaign=GH_Cookies_0708-rsa_a"
                    .to_string()
            ]
        );
        let rsa = ad.responsive_search_ad.as_ref().expect("rsa");
        let texts: Vec<&str> = rsa.headlines.iter().map(|h| h.text.as_str()).collect();
        assert_eq!(
            texts,
            vec!["Stop Cookie Pop-Ups", "Ghostery Ad Blocker", "Try Ghostery Free"]
        );
        assert_eq!(rsa.headlines[2].pin.as_deref(), Some("HEADLINE_1"));
    }
}

