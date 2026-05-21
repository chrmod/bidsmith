use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

use hcl_edit::Span;
use hcl_edit::expr::{Expression, Traversal, TraversalOperator};
use hcl_edit::structure::{Block, Body, Structure};
use serde::Serialize;

use crate::diagnostics::Diag;
use crate::parser::ParsedFile;

#[derive(Clone)]
pub enum FieldType {
    String,
    Integer,
    Number,
    Bool,
    Enum(&'static [&'static str]),
    Ref(&'static [&'static str]),
    RefOrResourceName(&'static [&'static str]),
    List(Box<FieldType>),
    RsaAssetList,
}

impl FieldType {
    pub fn list_of(inner: FieldType) -> Self {
        FieldType::List(Box::new(inner))
    }
}

pub struct AttributeSchema {
    pub name: &'static str,
    pub ty: FieldType,
    pub required: bool,
}

pub struct NestedBlockSchema {
    pub name: &'static str,
    pub schema: BlockSchema,
}

pub struct BlockSchema {
    pub attributes: Vec<AttributeSchema>,
    pub blocks: Vec<NestedBlockSchema>,
}

const STATUS: &[&str] = &["ENABLED", "PAUSED", "REMOVED"];
const KEYWORD_MATCH_TYPE: &[&str] = &["EXACT", "PHRASE", "BROAD"];
const RSA_PIN: &[&str] = &[
    "HEADLINE_1",
    "HEADLINE_2",
    "HEADLINE_3",
    "DESCRIPTION_1",
    "DESCRIPTION_2",
];
const PROXIMITY_RADIUS_UNITS: &[&str] = &["MILES", "KILOMETERS"];
const CONVERSION_ACTION_TYPE: &[&str] = &[
    "UNKNOWN",
    "AD_CALL",
    "ANDROID_APP_PRE_REGISTRATION",
    "ANDROID_INSTALLS_ALL_OTHER_APPS",
    "CLICK_TO_CALL",
    "FIREBASE_ANDROID_CUSTOM",
    "FIREBASE_ANDROID_FIRST_OPEN",
    "FIREBASE_ANDROID_IN_APP_PURCHASE",
    "FIREBASE_IOS_CUSTOM",
    "FIREBASE_IOS_FIRST_OPEN",
    "FIREBASE_IOS_IN_APP_PURCHASE",
    "FLOODLIGHT_ACTION",
    "FLOODLIGHT_TRANSACTION",
    "GOOGLE_ANALYTICS_4_CUSTOM",
    "GOOGLE_ANALYTICS_4_PURCHASE",
    "GOOGLE_HOSTED",
    "GOOGLE_PLAY_DOWNLOAD",
    "GOOGLE_PLAY_IN_APP_PURCHASE",
    "LEAD_FORM_SUBMIT",
    "SALESFORCE",
    "SEARCH_ADS_360",
    "SMART_CAMPAIGN_AD_CLICKS_TO_CALL",
    "SMART_CAMPAIGN_MAP_CLICKS_TO_CALL",
    "SMART_CAMPAIGN_MAP_DIRECTIONS",
    "SMART_CAMPAIGN_TRACKED_CALLS",
    "STORE_SALES",
    "STORE_SALES_DIRECT_UPLOAD",
    "STORE_VISITS",
    "THIRD_PARTY_APP_ANALYTICS_ANDROID_CUSTOM",
    "THIRD_PARTY_APP_ANALYTICS_ANDROID_FIRST_OPEN",
    "THIRD_PARTY_APP_ANALYTICS_ANDROID_IN_APP_PURCHASE",
    "THIRD_PARTY_APP_ANALYTICS_IOS_CUSTOM",
    "THIRD_PARTY_APP_ANALYTICS_IOS_FIRST_OPEN",
    "THIRD_PARTY_APP_ANALYTICS_IOS_IN_APP_PURCHASE",
    "UPLOAD_CALLS",
    "UPLOAD_CLICKS",
    "WEBPAGE",
    "WEBPAGE_CODELESS",
    "WEBSITE_CALL",
];
const CONVERSION_ACTION_CATEGORY: &[&str] = &[
    "DEFAULT",
    "PAGE_VIEW",
    "PURCHASE",
    "SIGNUP",
    "LEAD",
    "DOWNLOAD",
    "ADD_TO_CART",
    "BEGIN_CHECKOUT",
    "SUBSCRIBE_PAID",
    "PHONE_CALL_LEAD",
    "IMPORTED_LEAD",
    "SUBMIT_LEAD_FORM",
    "BOOK_APPOINTMENT",
    "REQUEST_QUOTE",
    "GET_DIRECTIONS",
    "OUTBOUND_CLICK",
    "CONTACT",
    "ENGAGEMENT",
    "STORE_VISIT",
    "STORE_SALE",
    "QUALIFIED_LEAD",
    "CONVERTED_LEAD",
];
const CONVERSION_ACTION_STATUS: &[&str] = &["ENABLED", "REMOVED", "HIDDEN"];
const CONVERSION_COUNTING_TYPE: &[&str] = &["ONE_PER_CLICK", "MANY_PER_CLICK"];
const CALL_CONVERSION_REPORTING_STATE: &[&str] = &[
    "DISABLED",
    "USE_ACCOUNT_LEVEL_CALL_CONVERSION_ACTION",
    "USE_RESOURCE_LEVEL_CALL_CONVERSION_ACTION",
];
const SHARED_SET_TYPE: &[&str] = &["NEGATIVE_KEYWORDS", "ACCOUNT_LEVEL_NEGATIVE_KEYWORDS"];
const SHARED_SET_STATUS: &[&str] = &["ENABLED", "REMOVED"];
const CAMPAIGN_SHARED_SET_STATUS: &[&str] = &["ENABLED", "REMOVED"];

const ASSET_FIELD_TYPE: &[&str] = &[
    "HEADLINE",
    "DESCRIPTION",
    "MANDATORY_AD_TEXT",
    "MARKETING_IMAGE",
    "MEDIA_BUNDLE",
    "YOUTUBE_VIDEO",
    "BOOK_ON_GOOGLE",
    "LEAD_FORM",
    "PROMOTION",
    "CALLOUT",
    "STRUCTURED_SNIPPET",
    "SITELINK",
    "MOBILE_APP",
    "HOTEL_CALLOUT",
    "CALL",
    "PRICE",
    "LONG_HEADLINE",
    "BUSINESS_NAME",
    "SQUARE_MARKETING_IMAGE",
    "PORTRAIT_MARKETING_IMAGE",
    "LOGO",
    "LANDSCAPE_LOGO",
    "VIDEO",
    "CALL_TO_ACTION_SELECTOR",
    "AD_IMAGE",
    "BUSINESS_LOGO",
    "HOTEL_PROPERTY",
    "DISCOVERY_CAROUSEL_CARD",
];

fn rsa_asset_block(name: &'static str) -> NestedBlockSchema {
    NestedBlockSchema {
        name,
        schema: BlockSchema {
            attributes: vec![
                attr("text", FieldType::String, true),
                attr("pin", FieldType::Enum(RSA_PIN), false),
            ],
            blocks: vec![],
        },
    }
}

fn keyword_block() -> NestedBlockSchema {
    NestedBlockSchema {
        name: "keyword",
        schema: BlockSchema {
            attributes: vec![
                attr("text", FieldType::String, true),
                attr("match_type", FieldType::Enum(KEYWORD_MATCH_TYPE), true),
            ],
            blocks: vec![],
        },
    }
}

fn attr(name: &'static str, ty: FieldType, required: bool) -> AttributeSchema {
    AttributeSchema { name, ty, required }
}

fn resource_schemas() -> &'static HashMap<&'static str, BlockSchema> {
    static SCHEMAS: OnceLock<HashMap<&'static str, BlockSchema>> = OnceLock::new();
    SCHEMAS.get_or_init(|| {
        let mut m = HashMap::new();

        m.insert(
            "google_ads_campaign_budget",
            BlockSchema {
                attributes: vec![
                    attr("name", FieldType::String, true),
                    attr("amount_micros", FieldType::Integer, true),
                    attr(
                        "delivery_method",
                        FieldType::Enum(&["STANDARD", "ACCELERATED"]),
                        false,
                    ),
                    attr("explicitly_shared", FieldType::Bool, false),
                ],
                blocks: vec![],
            },
        );

        m.insert(
            "google_ads_campaign",
            BlockSchema {
                attributes: vec![
                    attr("name", FieldType::String, true),
                    attr("status", FieldType::Enum(STATUS), false),
                    attr(
                        "advertising_channel_type",
                        FieldType::Enum(&[
                            "SEARCH",
                            "DISPLAY",
                            "SHOPPING",
                            "VIDEO",
                            "PERFORMANCE_MAX",
                            "MULTI_CHANNEL",
                            "LOCAL",
                            "SMART",
                            "DISCOVERY",
                            "DEMAND_GEN",
                        ]),
                        true,
                    ),
                    attr(
                        "campaign_budget",
                        FieldType::Ref(&["google_ads_campaign_budget"]),
                        true,
                    ),
                    attr(
                        "contains_eu_political_advertising",
                        FieldType::Enum(&[
                            "DOES_NOT_CONTAIN_EU_POLITICAL_ADVERTISING",
                            "CONTAINS_EU_POLITICAL_ADVERTISING",
                        ]),
                        false,
                    ),
                ],
                blocks: vec![
                    NestedBlockSchema {
                        name: "manual_cpc",
                        schema: BlockSchema {
                            attributes: vec![attr(
                                "enhanced_cpc_enabled",
                                FieldType::Bool,
                                false,
                            )],
                            blocks: vec![],
                        },
                    },
                    NestedBlockSchema {
                        name: "network_settings",
                        schema: BlockSchema {
                            attributes: vec![
                                attr("target_google_search", FieldType::Bool, false),
                                attr("target_search_network", FieldType::Bool, false),
                                attr("target_content_network", FieldType::Bool, false),
                                attr(
                                    "target_partner_search_network",
                                    FieldType::Bool,
                                    false,
                                ),
                            ],
                            blocks: vec![],
                        },
                    },
                ],
            },
        );

        m.insert(
            "google_ads_ad_group_criterion",
            BlockSchema {
                attributes: vec![
                    attr(
                        "ad_group",
                        FieldType::Ref(&["google_ads_ad_group"]),
                        true,
                    ),
                    attr("status", FieldType::Enum(STATUS), false),
                    attr("negative", FieldType::Bool, false),
                    attr("cpc_bid_micros", FieldType::Integer, false),
                ],
                blocks: vec![
                    keyword_block(),
                    NestedBlockSchema {
                        name: "negative_keyword",
                        schema: BlockSchema {
                            attributes: vec![
                                attr("text", FieldType::String, true),
                                attr(
                                    "match_type",
                                    FieldType::Enum(KEYWORD_MATCH_TYPE),
                                    true,
                                ),
                            ],
                            blocks: vec![],
                        },
                    },
                ],
            },
        );

        m.insert(
            "google_ads_campaign_criterion",
            BlockSchema {
                attributes: vec![
                    attr(
                        "campaign",
                        FieldType::Ref(&["google_ads_campaign"]),
                        true,
                    ),
                    attr("status", FieldType::Enum(STATUS), false),
                    attr("negative", FieldType::Bool, false),
                ],
                blocks: vec![
                    keyword_block(),
                    NestedBlockSchema {
                        name: "negative_keyword",
                        schema: BlockSchema {
                            attributes: vec![
                                attr("text", FieldType::String, true),
                                attr(
                                    "match_type",
                                    FieldType::Enum(KEYWORD_MATCH_TYPE),
                                    true,
                                ),
                            ],
                            blocks: vec![],
                        },
                    },
                    NestedBlockSchema {
                        name: "location",
                        schema: BlockSchema {
                            attributes: vec![attr(
                                "geo_target_constant",
                                FieldType::String,
                                true,
                            )],
                            blocks: vec![],
                        },
                    },
                    NestedBlockSchema {
                        name: "language",
                        schema: BlockSchema {
                            attributes: vec![attr(
                                "language_constant",
                                FieldType::String,
                                true,
                            )],
                            blocks: vec![],
                        },
                    },
                    NestedBlockSchema {
                        name: "proximity",
                        schema: BlockSchema {
                            attributes: vec![
                                attr("latitude", FieldType::Number, true),
                                attr("longitude", FieldType::Number, true),
                                attr("radius", FieldType::Number, true),
                                attr(
                                    "radius_units",
                                    FieldType::Enum(PROXIMITY_RADIUS_UNITS),
                                    true,
                                ),
                            ],
                            blocks: vec![],
                        },
                    },
                ],
            },
        );

        m.insert(
            "google_ads_ad_group_ad",
            BlockSchema {
                attributes: vec![
                    attr(
                        "ad_group",
                        FieldType::Ref(&["google_ads_ad_group"]),
                        true,
                    ),
                    attr("status", FieldType::Enum(STATUS), false),
                ],
                blocks: vec![NestedBlockSchema {
                    name: "ad",
                    schema: BlockSchema {
                        attributes: vec![
                            attr("name", FieldType::String, false),
                            attr(
                                "final_urls",
                                FieldType::list_of(FieldType::String),
                                true,
                            ),
                        ],
                        blocks: vec![NestedBlockSchema {
                            name: "responsive_search_ad",
                            schema: BlockSchema {
                                attributes: vec![
                                    attr("path1", FieldType::String, false),
                                    attr("path2", FieldType::String, false),
                                    attr("headlines", FieldType::RsaAssetList, false),
                                    attr("descriptions", FieldType::RsaAssetList, false),
                                ],
                                blocks: vec![
                                    rsa_asset_block("headline"),
                                    rsa_asset_block("description"),
                                ],
                            },
                        }],
                    },
                }],
            },
        );

        m.insert(
            "google_ads_ad_group",
            BlockSchema {
                attributes: vec![
                    attr("name", FieldType::String, true),
                    attr(
                        "campaign",
                        FieldType::Ref(&["google_ads_campaign"]),
                        true,
                    ),
                    attr("status", FieldType::Enum(STATUS), false),
                    attr(
                        "type",
                        FieldType::Enum(&[
                            "SEARCH_STANDARD",
                            "DISPLAY_STANDARD",
                            "SHOPPING_PRODUCT_ADS",
                            "VIDEO_BUMPER",
                            "VIDEO_TRUE_VIEW_IN_STREAM",
                            "VIDEO_TRUE_VIEW_IN_DISPLAY",
                        ]),
                        false,
                    ),
                    attr("cpc_bid_micros", FieldType::Integer, false),
                ],
                blocks: vec![],
            },
        );

        m.insert(
            "google_ads_conversion_action",
            BlockSchema {
                attributes: vec![
                    attr("name", FieldType::String, true),
                    attr(
                        "type",
                        FieldType::Enum(CONVERSION_ACTION_TYPE),
                        true,
                    ),
                    attr(
                        "category",
                        FieldType::Enum(CONVERSION_ACTION_CATEGORY),
                        true,
                    ),
                    attr(
                        "status",
                        FieldType::Enum(CONVERSION_ACTION_STATUS),
                        false,
                    ),
                    attr(
                        "counting_type",
                        FieldType::Enum(CONVERSION_COUNTING_TYPE),
                        false,
                    ),
                    attr(
                        "click_through_lookback_window_days",
                        FieldType::Integer,
                        false,
                    ),
                    attr(
                        "view_through_lookback_window_days",
                        FieldType::Integer,
                        false,
                    ),
                ],
                blocks: vec![NestedBlockSchema {
                    name: "value_settings",
                    schema: BlockSchema {
                        attributes: vec![
                            attr("default_value", FieldType::Number, false),
                            attr("default_currency_code", FieldType::String, false),
                            attr("always_use_default_value", FieldType::Bool, false),
                        ],
                        blocks: vec![],
                    },
                }],
            },
        );

        m.insert(
            "google_ads_call_asset",
            BlockSchema {
                attributes: vec![
                    attr("country_code", FieldType::String, true),
                    attr("phone_number", FieldType::String, true),
                    attr(
                        "call_conversion_reporting_state",
                        FieldType::Enum(CALL_CONVERSION_REPORTING_STATE),
                        false,
                    ),
                    attr(
                        "call_conversion_action",
                        FieldType::RefOrResourceName(&["google_ads_conversion_action"]),
                        false,
                    ),
                ],
                blocks: vec![],
            },
        );

        m.insert(
            "google_ads_shared_set",
            BlockSchema {
                attributes: vec![
                    attr("name", FieldType::String, true),
                    attr("type", FieldType::Enum(SHARED_SET_TYPE), false),
                    attr("status", FieldType::Enum(SHARED_SET_STATUS), false),
                ],
                blocks: vec![NestedBlockSchema {
                    name: "negative_keyword",
                    schema: BlockSchema {
                        attributes: vec![
                            attr("text", FieldType::String, true),
                            attr(
                                "match_type",
                                FieldType::Enum(KEYWORD_MATCH_TYPE),
                                true,
                            ),
                        ],
                        blocks: vec![],
                    },
                }],
            },
        );

        m.insert(
            "google_ads_shared_criterion",
            BlockSchema {
                attributes: vec![attr(
                    "shared_set",
                    FieldType::RefOrResourceName(&["google_ads_shared_set"]),
                    true,
                )],
                blocks: vec![keyword_block()],
            },
        );

        m.insert(
            "google_ads_campaign_shared_set",
            BlockSchema {
                attributes: vec![
                    attr(
                        "campaign",
                        FieldType::RefOrResourceName(&["google_ads_campaign"]),
                        true,
                    ),
                    attr(
                        "shared_set",
                        FieldType::RefOrResourceName(&["google_ads_shared_set"]),
                        true,
                    ),
                    attr(
                        "status",
                        FieldType::Enum(CAMPAIGN_SHARED_SET_STATUS),
                        false,
                    ),
                ],
                blocks: vec![],
            },
        );

        m.insert(
            "google_ads_customer_asset",
            BlockSchema {
                attributes: vec![
                    attr(
                        "asset",
                        FieldType::Ref(&["google_ads_call_asset"]),
                        true,
                    ),
                    attr(
                        "field_type",
                        FieldType::Enum(ASSET_FIELD_TYPE),
                        true,
                    ),
                    attr("status", FieldType::Enum(STATUS), false),
                ],
                blocks: vec![],
            },
        );

        m
    })
}

fn provider_schemas() -> &'static HashMap<&'static str, BlockSchema> {
    static SCHEMAS: OnceLock<HashMap<&'static str, BlockSchema>> = OnceLock::new();
    SCHEMAS.get_or_init(|| {
        let mut m = HashMap::new();
        m.insert(
            "google_ads",
            BlockSchema {
                attributes: vec![
                    attr("customer_id", FieldType::String, true),
                    attr("login_customer_id", FieldType::String, false),
                ],
                blocks: vec![],
            },
        );
        m
    })
}

struct ResourceDecl {
    file: String,
    module: String,
}

#[derive(Default)]
pub struct ResourceRegistry {
    by_qualified: HashMap<String, ResourceDecl>,
    by_short: HashMap<String, Vec<String>>,
}

impl ResourceRegistry {
    pub fn qualified(module: &str, ty: &str, name: &str) -> String {
        format!("{module}.{ty}.{name}")
    }

    fn short(ty: &str, name: &str) -> String {
        format!("{ty}.{name}")
    }

    pub fn resolve(&self, module: &str, ty: &str, name: &str) -> Resolution<'_> {
        let qualified = Self::qualified(module, ty, name);
        if self.by_qualified.contains_key(&qualified) {
            return Resolution::Found(qualified);
        }
        let short = Self::short(ty, name);
        match self.by_short.get(&short).map(Vec::as_slice).unwrap_or(&[]) {
            [] => Resolution::Missing,
            [only] => Resolution::Found(Self::qualified(only, ty, name)),
            many => Resolution::Ambiguous(many),
        }
    }

    pub fn build(files: &[ParsedFile]) -> (Self, Vec<Diag>) {
        let mut registry = ResourceRegistry::default();
        let mut diags = Vec::new();
        for f in files {
            for s in f.body.iter() {
                let Structure::Block(b) = s else { continue };
                if b.ident.as_str() != "resource" || b.labels.len() != 2 {
                    continue;
                }
                let ty = b.labels[0].as_str();
                let name = b.labels[1].as_str();
                let qualified = Self::qualified(&f.module, ty, name);
                if let Some(prev) = registry.by_qualified.get(&qualified) {
                    let short = Self::short(ty, name);
                    let extra = if prev.module == f.module {
                        String::new()
                    } else {
                        format!(" (module '{}')", f.module)
                    };
                    diags.push(Diag::new(
                        f.src.clone(),
                        block_span(b),
                        format!(
                            "duplicate resource '{short}'{extra} (also declared at {})",
                            prev.file
                        ),
                    ));
                    continue;
                }
                registry.by_qualified.insert(
                    qualified,
                    ResourceDecl {
                        file: f.path.display().to_string(),
                        module: f.module.clone(),
                    },
                );
                registry
                    .by_short
                    .entry(Self::short(ty, name))
                    .or_default()
                    .push(f.module.clone());
            }
        }
        (registry, diags)
    }
}

pub enum Resolution<'a> {
    Found(String),
    Missing,
    Ambiguous(&'a [String]),
}

pub fn validate_files(files: &[ParsedFile]) -> Vec<Diag> {
    let (registry, mut diags) = ResourceRegistry::build(files);

    for f in files {
        validate_top_level(f, &registry, &mut diags);
    }

    diags.sort_by(|a, b| {
        (a.src.name(), a.span.offset()).cmp(&(b.src.name(), b.span.offset()))
    });
    diags
}

fn validate_top_level(
    file: &ParsedFile,
    registry: &ResourceRegistry,
    diags: &mut Vec<Diag>,
) {
    for s in file.body.iter() {
        match s {
            Structure::Attribute(a) => {
                diags.push(Diag::new(
                    file.src.clone(),
                    span_of(a.key.span()),
                    format!(
                        "top-level attributes are not allowed; place '{}' inside a 'provider' or 'resource' block",
                        a.key.as_str()
                    ),
                ));
            }
            Structure::Block(b) => match b.ident.as_str() {
                "provider" => validate_provider(file, b, registry, diags),
                "resource" => validate_resource(file, b, registry, diags),
                other => {
                    diags.push(Diag::new(
                        file.src.clone(),
                        span_of(b.ident.span()),
                        format!(
                            "unknown top-level block '{other}'; expected 'provider' or 'resource'"
                        ),
                    ));
                }
            },
        }
    }
}

fn validate_provider(
    file: &ParsedFile,
    block: &Block,
    registry: &ResourceRegistry,
    diags: &mut Vec<Diag>,
) {
    if block.labels.len() != 1 {
        diags.push(Diag::new(
            file.src.clone(),
            span_of(block.ident.span()),
            format!(
                "'provider' block requires exactly one label (the provider name), got {}",
                block.labels.len()
            ),
        ));
        return;
    }
    let provider_name = block.labels[0].as_str();
    let schema = match provider_schemas().get(provider_name) {
        Some(s) => s,
        None => {
            diags.push(Diag::new(
                file.src.clone(),
                span_of(block.labels[0].span()),
                format!("unknown provider '{provider_name}'"),
            ));
            return;
        }
    };
    validate_body(
        file,
        block,
        &block.body,
        schema,
        &format!("provider.{provider_name}"),
        registry,
        diags,
    );
}

fn validate_resource(
    file: &ParsedFile,
    block: &Block,
    registry: &ResourceRegistry,
    diags: &mut Vec<Diag>,
) {
    if block.labels.len() != 2 {
        diags.push(Diag::new(
            file.src.clone(),
            span_of(block.ident.span()),
            format!(
                "'resource' block requires exactly two labels (type and name), got {}",
                block.labels.len()
            ),
        ));
        return;
    }
    let ty = block.labels[0].as_str();
    let name = block.labels[1].as_str();
    let schema = match resource_schemas().get(ty) {
        Some(s) => s,
        None => {
            diags.push(Diag::new(
                file.src.clone(),
                span_of(block.labels[0].span()),
                format!("unknown resource type '{ty}'"),
            ));
            return;
        }
    };
    validate_body(
        file,
        block,
        &block.body,
        schema,
        &format!("{ty}.{name}"),
        registry,
        diags,
    );
}

fn validate_body(
    file: &ParsedFile,
    containing: &Block,
    body: &Body,
    schema: &BlockSchema,
    address: &str,
    registry: &ResourceRegistry,
    diags: &mut Vec<Diag>,
) {
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();

    for s in body.iter() {
        match s {
            Structure::Attribute(a) => {
                let key = a.key.as_str();
                if !seen.insert(key) {
                    diags.push(Diag::new(
                        file.src.clone(),
                        span_of(a.key.span()),
                        format!("duplicate attribute '{key}' in {address}"),
                    ));
                    continue;
                }
                let Some(attr_schema) = schema.attributes.iter().find(|x| x.name == key) else {
                    diags.push(Diag::new(
                        file.src.clone(),
                        span_of(a.key.span()),
                        format!("unknown attribute '{key}' in {address}"),
                    ));
                    continue;
                };
                validate_value(file, &a.value, &attr_schema.ty, registry, diags);
            }
            Structure::Block(b) => {
                let bname = b.ident.as_str();
                let Some(sub_schema) = schema.blocks.iter().find(|x| x.name == bname) else {
                    diags.push(Diag::new(
                        file.src.clone(),
                        span_of(b.ident.span()),
                        format!("unknown nested block '{bname}' in {address}"),
                    ));
                    continue;
                };
                if !b.labels.is_empty() {
                    diags.push(Diag::new(
                        file.src.clone(),
                        span_of(b.labels[0].span()),
                        format!("nested block '{bname}' does not take labels"),
                    ));
                }
                validate_body(
                    file,
                    b,
                    &b.body,
                    &sub_schema.schema,
                    &format!("{address}.{bname}"),
                    registry,
                    diags,
                );
            }
        }
    }

    for a in &schema.attributes {
        if a.required && !seen.contains(a.name) {
            diags.push(Diag::new(
                file.src.clone(),
                span_of(containing.ident.span()),
                format!("missing required attribute '{}' in {}", a.name, address),
            ));
        }
    }
}

fn validate_value(
    file: &ParsedFile,
    expr: &Expression,
    ty: &FieldType,
    registry: &ResourceRegistry,
    diags: &mut Vec<Diag>,
) {
    let span = span_of(expr.span());
    match ty {
        FieldType::String => {
            if !matches!(expr, Expression::String(_)) {
                diags.push(Diag::new(
                    file.src.clone(),
                    span,
                    format!("expected string, got {}", describe_expr(expr)),
                ));
            }
        }
        FieldType::Number => {
            if !matches!(expr, Expression::Number(_)) {
                diags.push(Diag::new(
                    file.src.clone(),
                    span,
                    format!("expected number, got {}", describe_expr(expr)),
                ));
            }
        }
        FieldType::Integer => match expr {
            Expression::Number(n) => {
                let formatted = &**n;
                if formatted.as_f64().map(|f| f.fract() != 0.0).unwrap_or(false) {
                    diags.push(Diag::new(
                        file.src.clone(),
                        span,
                        format!("expected integer, got fractional number {formatted}"),
                    ));
                }
            }
            other => diags.push(Diag::new(
                file.src.clone(),
                span,
                format!("expected integer, got {}", describe_expr(other)),
            )),
        },
        FieldType::Bool => {
            if !matches!(expr, Expression::Bool(_)) {
                diags.push(Diag::new(
                    file.src.clone(),
                    span,
                    format!("expected boolean, got {}", describe_expr(expr)),
                ));
            }
        }
        FieldType::Enum(values) => match expr {
            Expression::String(s) => {
                let v = s.as_str();
                if !values.iter().any(|&x| x == v) {
                    diags.push(Diag::new(
                        file.src.clone(),
                        span,
                        format!(
                            "invalid value \"{v}\"; expected one of [{}]",
                            values.join(", ")
                        ),
                    ));
                }
            }
            other => diags.push(Diag::new(
                file.src.clone(),
                span,
                format!(
                    "expected one of [{}], got {}",
                    values.join(", "),
                    describe_expr(other)
                ),
            )),
        },
        FieldType::List(inner) => match expr {
            Expression::Array(arr) => {
                for item in arr.iter() {
                    validate_value(file, item, inner, registry, diags);
                }
            }
            other => diags.push(Diag::new(
                file.src.clone(),
                span,
                format!(
                    "expected list of {}, got {}",
                    describe_field_type(inner),
                    describe_expr(other)
                ),
            )),
        },
        FieldType::RsaAssetList => match expr {
            Expression::Array(arr) => {
                for item in arr.iter() {
                    validate_rsa_asset_item(file, item, diags);
                }
            }
            other => diags.push(Diag::new(
                file.src.clone(),
                span,
                format!(
                    "expected list of strings or {{ text, pin? }} objects, got {}",
                    describe_expr(other)
                ),
            )),
        },
        FieldType::Ref(targets) => {
            validate_ref(file, expr, span, targets, registry, diags, false);
        }
        FieldType::RefOrResourceName(targets) => {
            if matches!(expr, Expression::String(_)) {
                return;
            }
            validate_ref(file, expr, span, targets, registry, diags, true);
        }
    }
}

fn validate_ref(
    file: &ParsedFile,
    expr: &Expression,
    span: std::ops::Range<usize>,
    targets: &[&str],
    registry: &ResourceRegistry,
    diags: &mut Vec<Diag>,
    allow_resource_name: bool,
) {
    let expected = if allow_resource_name {
        format!("reference to {} or resource-name string", join_or(targets))
    } else {
        format!("reference to {}", join_or(targets))
    };
    let Expression::Traversal(t) = expr else {
        diags.push(Diag::new(
            file.src.clone(),
            span,
            format!("expected {expected}, got {}", describe_expr(expr)),
        ));
        return;
    };
    let Some(path) = extract_traversal_path(t) else {
        diags.push(Diag::new(
            file.src.clone(),
            span,
            "unsupported reference expression (only `<type>.<name>.<attribute>` is allowed)"
                .to_string(),
        ));
        return;
    };
    if path.len() < 2 {
        diags.push(Diag::new(
            file.src.clone(),
            span,
            format!(
                "incomplete reference '{}'; expected '<type>.<name>.<attribute>'",
                path.join(".")
            ),
        ));
        return;
    }
    let ref_type = &path[0];
    let ref_name = &path[1];
    if !targets.iter().any(|&t| t == ref_type) {
        diags.push(Diag::new(
            file.src.clone(),
            span,
            format!(
                "expected {expected}, got reference to '{}'",
                ref_type
            ),
        ));
        return;
    }
    let address = format!("{ref_type}.{ref_name}");
    match registry.resolve(&file.module, ref_type, ref_name) {
        Resolution::Found(_) => {}
        Resolution::Missing => {
            diags.push(Diag::new(
                file.src.clone(),
                span,
                format!("reference to undeclared resource '{address}'"),
            ));
        }
        Resolution::Ambiguous(modules) => {
            let mut sorted: Vec<&str> = modules.iter().map(String::as_str).collect();
            sorted.sort();
            diags.push(Diag::new(
                file.src.clone(),
                span,
                format!(
                    "ambiguous reference to '{address}'; declared in modules [{}] — rename one of the resources so each is unique within its module",
                    sorted.join(", ")
                ),
            ));
        }
    }
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

fn describe_field_type(ty: &FieldType) -> String {
    match ty {
        FieldType::String => "string".to_string(),
        FieldType::Integer => "integer".to_string(),
        FieldType::Number => "number".to_string(),
        FieldType::Bool => "boolean".to_string(),
        FieldType::Enum(values) => format!("one of [{}]", values.join(", ")),
        FieldType::Ref(targets) => format!("reference to {}", join_or(targets)),
        FieldType::RefOrResourceName(targets) => format!(
            "reference to {} or resource-name string",
            join_or(targets)
        ),
        FieldType::List(inner) => format!("list of {}", describe_field_type(inner)),
        FieldType::RsaAssetList => "list of strings or { text, pin? } objects".to_string(),
    }
}

fn validate_rsa_asset_item(file: &ParsedFile, expr: &Expression, diags: &mut Vec<Diag>) {
    let span = span_of(expr.span());
    match expr {
        Expression::String(_) => {}
        Expression::Object(obj) => {
            let mut has_text = false;
            for (key, value) in obj.iter() {
                let Some(ident) = key.as_ident() else {
                    diags.push(Diag::new(
                        file.src.clone(),
                        span.clone(),
                        "RSA asset object keys must be identifiers ('text' or 'pin')".to_string(),
                    ));
                    continue;
                };
                match ident.as_str() {
                    "text" => {
                        has_text = true;
                        if !matches!(value.expr(), Expression::String(_)) {
                            diags.push(Diag::new(
                                file.src.clone(),
                                span_of(value.expr().span()),
                                format!(
                                    "RSA asset 'text' must be a string, got {}",
                                    describe_expr(value.expr())
                                ),
                            ));
                        }
                    }
                    "pin" => match value.expr() {
                        Expression::String(s) => {
                            let v = s.as_str();
                            if !RSA_PIN.iter().any(|&x| x == v) {
                                diags.push(Diag::new(
                                    file.src.clone(),
                                    span_of(value.expr().span()),
                                    format!(
                                        "invalid pin \"{v}\"; expected one of [{}]",
                                        RSA_PIN.join(", ")
                                    ),
                                ));
                            }
                        }
                        other => diags.push(Diag::new(
                            file.src.clone(),
                            span_of(value.expr().span()),
                            format!(
                                "RSA asset 'pin' must be a string, got {}",
                                describe_expr(other)
                            ),
                        )),
                    },
                    other => {
                        diags.push(Diag::new(
                            file.src.clone(),
                            span.clone(),
                            format!(
                                "unknown key '{other}' in RSA asset object; allowed: text, pin"
                            ),
                        ));
                    }
                }
            }
            if !has_text {
                diags.push(Diag::new(
                    file.src.clone(),
                    span,
                    "RSA asset object is missing required key 'text'".to_string(),
                ));
            }
        }
        other => diags.push(Diag::new(
            file.src.clone(),
            span,
            format!(
                "expected RSA asset (string or {{ text, pin? }} object), got {}",
                describe_expr(other)
            ),
        )),
    }
}

fn describe_expr(expr: &Expression) -> String {
    match expr {
        Expression::String(s) => format!("string \"{}\"", s.as_str()),
        Expression::Number(n) => format!("number {}", **n),
        Expression::Bool(b) => format!("boolean {}", **b),
        Expression::Traversal(t) => match extract_traversal_path(t) {
            Some(p) => format!("reference '{}'", p.join(".")),
            None => "expression".to_string(),
        },
        Expression::Variable(v) => format!("identifier '{}'", v.as_str()),
        Expression::Array(_) => "array".to_string(),
        Expression::Object(_) => "object".to_string(),
        Expression::Null(_) => "null".to_string(),
        _ => "expression".to_string(),
    }
}

fn join_or(items: &[&str]) -> String {
    items.join(" or ")
}

fn block_span(b: &Block) -> std::ops::Range<usize> {
    let start = b.ident.span().map(|r| r.start).unwrap_or(0);
    let end = b
        .body
        .span()
        .map(|r| r.end)
        .or_else(|| b.labels.last().and_then(|l| l.span().map(|r| r.end)))
        .unwrap_or(start);
    start..end
}

fn span_of(s: Option<std::ops::Range<usize>>) -> std::ops::Range<usize> {
    s.unwrap_or(0..0)
}

#[derive(Serialize)]
pub struct SchemaDoc {
    pub version: &'static str,
    pub providers: BTreeMap<&'static str, BlockDoc>,
    pub resources: BTreeMap<&'static str, BlockDoc>,
}

#[derive(Serialize)]
pub struct BlockDoc {
    pub attributes: Vec<AttributeDoc>,
    pub blocks: Vec<NestedBlockDoc>,
}

#[derive(Serialize)]
pub struct AttributeDoc {
    pub name: &'static str,
    pub required: bool,
    #[serde(flatten)]
    pub ty: TypeDoc,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TypeDoc {
    String,
    Integer,
    Number,
    Boolean,
    Enum { values: Vec<&'static str> },
    Reference { targets: Vec<&'static str> },
    ReferenceOrResourceName { targets: Vec<&'static str> },
    List { element: Box<TypeDoc> },
    RsaAssetList,
}

#[derive(Serialize)]
pub struct NestedBlockDoc {
    pub name: &'static str,
    pub attributes: Vec<AttributeDoc>,
    pub blocks: Vec<NestedBlockDoc>,
}

pub fn dump_schema() -> SchemaDoc {
    let providers = provider_schemas()
        .iter()
        .map(|(&k, v)| (k, block_to_doc(v)))
        .collect();
    let resources = resource_schemas()
        .iter()
        .map(|(&k, v)| (k, block_to_doc(v)))
        .collect();
    SchemaDoc {
        version: env!("CARGO_PKG_VERSION"),
        providers,
        resources,
    }
}

fn block_to_doc(b: &BlockSchema) -> BlockDoc {
    BlockDoc {
        attributes: b.attributes.iter().map(attr_to_doc).collect(),
        blocks: b.blocks.iter().map(nested_block_to_doc).collect(),
    }
}

fn nested_block_to_doc(n: &NestedBlockSchema) -> NestedBlockDoc {
    NestedBlockDoc {
        name: n.name,
        attributes: n.schema.attributes.iter().map(attr_to_doc).collect(),
        blocks: n.schema.blocks.iter().map(nested_block_to_doc).collect(),
    }
}

fn attr_to_doc(a: &AttributeSchema) -> AttributeDoc {
    AttributeDoc {
        name: a.name,
        required: a.required,
        ty: ty_to_doc(&a.ty),
    }
}

fn ty_to_doc(ty: &FieldType) -> TypeDoc {
    match ty {
        FieldType::String => TypeDoc::String,
        FieldType::Integer => TypeDoc::Integer,
        FieldType::Number => TypeDoc::Number,
        FieldType::Bool => TypeDoc::Boolean,
        FieldType::Enum(values) => TypeDoc::Enum {
            values: values.to_vec(),
        },
        FieldType::Ref(targets) => TypeDoc::Reference {
            targets: targets.to_vec(),
        },
        FieldType::RefOrResourceName(targets) => TypeDoc::ReferenceOrResourceName {
            targets: targets.to_vec(),
        },
        FieldType::List(inner) => TypeDoc::List {
            element: Box::new(ty_to_doc(inner)),
        },
        FieldType::RsaAssetList => TypeDoc::RsaAssetList,
    }
}
